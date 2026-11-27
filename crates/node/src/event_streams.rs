//! The event and presence streams a node serves at once (`GET /apps/{app}/events`,
//! `GET /apps/{app}/presence/stream`), and the share of them one account and one app may
//! hold, so no caller takes every slot of a node from the others.
//!
//! Spec: [Node limits](https://calimero-network.github.io/core/protocol/limits/#node-limits)

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::MutexGuard;

/// The most event and presence streams one node serves at once.
pub const MAX_EVENT_STREAMS: usize = 4096;

/// The most event and presence streams one account holds open on one node, through its
/// sessions and its API keys together.
pub const MAX_EVENT_STREAMS_PER_ACCOUNT: usize = 64;

/// The most event and presence streams one app's subscribers hold open on one node: a
/// quarter of [`MAX_EVENT_STREAMS`], so the subscribers of one app leave room for the others.
pub const MAX_EVENT_STREAMS_PER_APP: usize = MAX_EVENT_STREAMS / 4;

/// Which share a refused stream would have gone past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    /// The account's ([`MAX_EVENT_STREAMS_PER_ACCOUNT`]).
    Account,
    /// The app's ([`MAX_EVENT_STREAMS_PER_APP`]).
    App,
    /// The node's ([`MAX_EVENT_STREAMS`]).
    Node,
}

#[derive(Default)]
struct Counts {
    total: usize,
    /// Only accounts holding at least one stream: at most [`MAX_EVENT_STREAMS`] entries.
    by_account: HashMap<String, usize>,
    /// Only apps with at least one stream: at most [`MAX_EVENT_STREAMS`] entries.
    by_app: HashMap<String, usize>,
}

/// The streams open on this node, in total, by account and by app.
#[derive(Default)]
pub struct EventStreams(Mutex<Counts>);

impl EventStreams {
    fn counts(&self) -> MutexGuard<'_, Counts> {
        // A panic while holding the lock leaves only counters, which stay consistent.
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The streams open on this node now.
    pub fn open(&self) -> usize {
        self.counts().total
    }

    /// A slot for one stream of `account` (`None`: a caller without an account, which an
    /// app any caller reaches admits; it counts toward the app's and the node's shares only)
    /// on `app`, held until the returned [`StreamSlot`] drops.
    ///
    /// # Errors
    /// The share the stream would go past, the account's judged first.
    pub fn take(self: &Arc<Self>, account: Option<&str>, app: &str) -> Result<StreamSlot, Full> {
        let mut c = self.counts();
        if let Some(a) = account {
            if c.by_account.get(a).copied().unwrap_or(0) >= MAX_EVENT_STREAMS_PER_ACCOUNT {
                return Err(Full::Account);
            }
        }
        if c.by_app.get(app).copied().unwrap_or(0) >= MAX_EVENT_STREAMS_PER_APP {
            return Err(Full::App);
        }
        if c.total >= MAX_EVENT_STREAMS {
            return Err(Full::Node);
        }
        c.total += 1;
        *c.by_app.entry(app.to_owned()).or_default() += 1;
        if let Some(a) = account {
            *c.by_account.entry(a.to_owned()).or_default() += 1;
        }
        Ok(StreamSlot {
            streams: Arc::clone(self),
            account: account.map(str::to_owned),
            app: app.to_owned(),
        })
    }
}

/// One open stream, counted until it drops, on whichever path the stream ends.
pub struct StreamSlot {
    streams: Arc<EventStreams>,
    account: Option<String>,
    app: String,
}

/// Takes one from `key`'s count, dropping the entry at zero.
fn release(map: &mut HashMap<String, usize>, key: &str) {
    let Some(n) = map.get_mut(key) else {
        return;
    };
    *n = n.saturating_sub(1);
    if *n == 0 {
        let _gone = map.remove(key);
    }
}

impl Drop for StreamSlot {
    fn drop(&mut self) {
        let mut c = self.streams.counts();
        c.total = c.total.saturating_sub(1);
        release(&mut c.by_app, &self.app);
        if let Some(a) = &self.account {
            release(&mut c.by_account, a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_account_at_its_share_leaves_slots_for_others_and_gets_them_back_on_drop() {
        let s = Arc::new(EventStreams::default());
        let mut held: Vec<_> = (0..MAX_EVENT_STREAMS_PER_ACCOUNT)
            .map(|_| s.take(Some("alice"), "app").expect("within the share"))
            .collect();
        assert_eq!(s.take(Some("alice"), "other").err(), Some(Full::Account));
        let _bob = s.take(Some("bob"), "app").expect("another account");
        let _anon = s.take(None, "app").expect("a caller without an account");
        drop(held.pop());
        let _again = s
            .take(Some("alice"), "app")
            .expect("the dropped slot is back");
        drop(held);
        drop(_again);
        assert!(!s.counts().by_account.contains_key("alice"));
        assert_eq!(s.open(), 2);
    }

    #[test]
    fn one_app_and_the_node_have_shares_of_their_own() {
        let s = Arc::new(EventStreams::default());
        let accounts: Vec<String> = (0..MAX_EVENT_STREAMS / MAX_EVENT_STREAMS_PER_ACCOUNT)
            .map(|n| format!("a{n}"))
            .collect();
        let mut held = Vec::new();
        for a in accounts
            .iter()
            .take(MAX_EVENT_STREAMS_PER_APP / MAX_EVENT_STREAMS_PER_ACCOUNT)
        {
            for _ in 0..MAX_EVENT_STREAMS_PER_ACCOUNT {
                held.push(s.take(Some(a), "busy").expect("within the app's share"));
            }
        }
        assert_eq!(s.take(Some("late"), "busy").err(), Some(Full::App));
        assert_eq!(s.take(None, "busy").err(), Some(Full::App));
        let busy = MAX_EVENT_STREAMS_PER_APP / MAX_EVENT_STREAMS_PER_ACCOUNT;
        for (n, a) in accounts.iter().enumerate().skip(busy) {
            for _ in 0..MAX_EVENT_STREAMS_PER_ACCOUNT {
                let app = format!("app{}", n % 4);
                held.push(s.take(Some(a), &app).expect("within every share"));
            }
        }
        assert_eq!(s.open(), MAX_EVENT_STREAMS);
        assert_eq!(s.take(Some("late"), "quiet").err(), Some(Full::Node));
    }
}
