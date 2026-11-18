//! What losing the node that issued a session, or a node on the wrong
//! release, does to a session.
//!
//! Since core #4774 a session token is signed by the node that issued it, and
//! the cluster's other nodes take it only while they can vouch for that node's
//! signing key. When the issuer is replaced, removed or out of reach (a rolling
//! deploy, an image rollout), they answer its tokens `401 UNAUTHENTICATED`:
//! the user did not sign out, and `POST /auth/refresh` on any node gives a
//! fresh session. A node whose mero-tee release the account's owners have not
//! approved answers `421 WRONG_NODE` instead: it neither issues nor takes the
//! account's sessions, so the client moves to another node rather than
//! refreshing there.
//!
//! [`Nodes::run`] answers both, once each per request:
//!
//! - a `401` to a request sent with a session: refresh once and repeat it. A
//!   refresh token is used once (a second use revokes the session), so the
//!   requests refused at the same time share one refresh ([`Nodes::renew`]).
//! - a `421 WRONG_NODE`: move to the next node the client was given and repeat
//!   the request there.
//!
//! Repeating is safe for any request, a call too: the node answers `401` and
//! `421` while judging the credential, before it runs anything, and answers
//! the ceremonies that mint a session `421` before it spends their challenge
//! or refresh token. A sign-in or sign-up goes through [`Nodes::run_once`]: it
//! needs the passkey again, so its caller repeats it. A refresh answered `421`
//! goes on to each next node with the same token, once per node.
//!
//! A node that is unavailable is left for the next node too, each node once
//! per request; after the last, the last error is the caller's:
//!
//! - one that could not be connected to ([`unreachable`]), by any request:
//!   nothing reached it.
//! - one that answers `503` or times out ([`unavailable`]), only by a request
//!   that is safe to send twice ([`Nodes::run_repeatable`]): a read, or a call,
//!   which carries the same idempotency key to every node. The key is recorded
//!   in the app's replicated log, so the cluster answers it once. Core's client
//!   first sends a call again to the same node for a while; it moves on once
//!   that gives up.
//!
//! Sign-in and sign-up ([`Nodes::run_once`]) and refreshes are never sent again
//! for this. A client made from the node directory (`Client.connect`) asks it
//! again for the cluster's nodes after such a move, and once the listing it has
//! is due ([`Nodes::with_directory`], [`merge`]).

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use calimero_client::{Error, Session};

/// What the recovery needs of one node's client. [`SealedNode`](super::sealed::SealedNode)
/// in the bindings; a stand-in in the tests below.
pub(crate) trait Node: Send + Sync {
    async fn session(&self) -> Option<Session>;
    async fn use_session(&self, session: Session);
    async fn use_api_key(&self, key: &str);
    async fn refresh(&self) -> Result<Session, Error>;
}

/// A listing of the cluster's nodes, in the order to use them, and how long
/// it holds.
pub(crate) type Listing = (Vec<String>, Duration);

/// Where a client made from the node directory (`Client.connect`) learns the
/// cluster's nodes again, and how it reaches one it did not know.
pub(crate) trait Directory<N>: Send + Sync {
    fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<Listing, String>> + Send + '_>>;
    /// A client for `url`, attested under the client's policy before use.
    fn connect(&self, url: &str) -> Result<N, String>;
}

/// `401 UNAUTHENTICATED`.
pub(crate) fn unauthenticated(e: &Error) -> bool {
    matches!(e, Error::Api { status: 401, code, .. } if code == "UNAUTHENTICATED")
}

/// `421 WRONG_NODE`, and not another 421 (`NOT_LEADER` names a leader to
/// follow). Core's client names it `Error::WrongNode` (core #4774).
pub(crate) fn wrong_node(e: &Error) -> bool {
    matches!(e, Error::WrongNode { .. })
        || matches!(e, Error::Api { status: 421, code, .. } if code == "WRONG_NODE")
}

/// The node could not be connected to, so nothing was sent to it: any
/// request may go to another node.
pub(crate) fn unreachable(e: &Error) -> bool {
    match e {
        Error::Transport(r) => r.chain().any(|c| {
            c.downcast_ref::<reqwest::Error>()
                .is_some_and(reqwest::Error::is_connect)
        }),
        Error::Api { .. } | Error::WrongNode { .. } => false,
    }
}

/// The node is unavailable (`503`, unreachable, timed out): a request that
/// is safe to send twice (a read, a call with its idempotency key) may go to
/// another node.
pub(crate) fn unavailable(e: &Error) -> bool {
    e.is_unavailable()
}

/// The nodes in the order to use them, and the one in use.
struct State<N> {
    list: Vec<(String, Arc<N>)>,
    at: usize,
}

/// The directory a client was made from, and when to ask it again.
struct Refresh<N> {
    source: Box<dyn Directory<N>>,
    due: std::sync::Mutex<Instant>,
    /// Held while fetching: one refresh at a time, the others skip it.
    busy: tokio::sync::Mutex<()>,
}

/// The nodes of one cluster a client may use, the one in use, and its credential.
pub(crate) struct Nodes<N> {
    state: RwLock<State<N>>,
    /// The API key last given to `use_api_key`, or `None` while a session is
    /// the credential. A request made with an API key is never refreshed.
    api_key: std::sync::Mutex<Option<String>>,
    /// Held for a refresh: whoever comes next sees the session it made.
    refreshing: tokio::sync::Mutex<()>,
    directory: Option<Refresh<N>>,
}

/// The node list after the directory answered `listed`: the node in use
/// first, then the directory's order. A node it no longer lists is dropped,
/// except the one in use.
pub(crate) fn merge(in_use: &str, listed: &[String]) -> Vec<String> {
    std::iter::once(in_use)
        .chain(listed.iter().map(String::as_str).filter(|u| *u != in_use))
        .map(str::to_owned)
        .collect()
}

impl<N: Node> Nodes<N> {
    /// `nodes` in the order to use them (`(url, its client)`), the first in use.
    pub(crate) fn new(nodes: Vec<(String, N)>) -> Self {
        Self {
            state: RwLock::new(State {
                list: nodes.into_iter().map(|(u, n)| (u, Arc::new(n))).collect(),
                at: 0,
            }),
            api_key: std::sync::Mutex::new(None),
            refreshing: tokio::sync::Mutex::new(()),
            directory: None,
        }
    }

    /// Learn the nodes again from `source` once `refresh_after` has passed,
    /// and soon after a request moved off an unavailable node.
    pub(crate) fn with_directory(
        mut self,
        source: Box<dyn Directory<N>>,
        refresh_after: Duration,
    ) -> Self {
        self.directory = Some(Refresh {
            source,
            due: std::sync::Mutex::new(Instant::now() + refresh_after),
            busy: tokio::sync::Mutex::new(()),
        });
        self
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, State<N>> {
        self.state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, State<N>> {
        self.state
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The node in use and its index.
    pub(crate) fn current(&self) -> (usize, Arc<N>) {
        let s = self.read();
        (s.at, Arc::clone(&s.list[s.at].1))
    }

    /// The URL of the node in use.
    pub(crate) fn url(&self) -> String {
        let s = self.read();
        s.list[s.at].0.clone()
    }

    /// Every node's URL, in the order they are used.
    pub(crate) fn urls(&self) -> Vec<String> {
        self.read().list.iter().map(|(u, _)| u.clone()).collect()
    }

    fn len(&self) -> usize {
        self.read().list.len()
    }

    pub(crate) fn api_key(&self) -> Option<String> {
        self.api_key.lock().ok().and_then(|k| k.clone())
    }

    /// Record the credential: `Some(key)` for an API key, `None` for a session.
    pub(crate) fn set_api_key(&self, key: Option<String>) {
        if let Ok(mut k) = self.api_key.lock() {
            *k = key;
        }
    }

    /// The session token a request sent to `node` now carries, if it carries one.
    async fn session_token(&self, node: &N) -> Option<String> {
        if self.api_key().is_some() {
            return None;
        }
        node.session().await.map(|s| s.session_token)
    }

    /// Move on from `from` (the node that refused or failed) to the next,
    /// handing it the credential. False when there is no other node. A
    /// request that saw `from` fail after another moved on already does not
    /// move again.
    async fn move_on(&self, from: &Arc<N>) -> bool {
        let (i, next) = {
            let s = self.read();
            if s.list.len() < 2 {
                return false;
            }
            match s.list.iter().position(|(_, n)| Arc::ptr_eq(n, from)) {
                Some(i) if i == s.at => (i, Arc::clone(&s.list[(i + 1) % s.list.len()].1)),
                // Moved already, or the list changed since: the node in use is another.
                _ => return true,
            }
        };
        if let Some(key) = self.api_key() {
            next.use_api_key(&key).await;
        } else if let Some(session) = from.session().await {
            next.use_session(session).await;
        }
        let mut s = self.write();
        if s.at == i && s.list.get(i).is_some_and(|(_, n)| Arc::ptr_eq(n, from)) {
            s.at = (i + 1) % s.list.len();
        }
        true
    }

    /// Run `op` on the node in use, answering one `401` with a refresh and one
    /// `421 WRONG_NODE` with the next node, each by repeating `op` once (see
    /// the module's documentation for why that is safe). A node that could
    /// not be reached is left for the next, each node tried once.
    pub(crate) async fn run<T>(
        &self,
        op: impl AsyncFn(Arc<N>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.run_with(op, unreachable).await
    }

    /// [`run`](Self::run) for a request that is safe to send twice (a read, a
    /// call with its idempotency key): a node that is unavailable (`503`, out
    /// of reach, timed out) is also left for the next.
    pub(crate) async fn run_repeatable<T>(
        &self,
        op: impl AsyncFn(Arc<N>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.run_with(op, unavailable).await
    }

    async fn run_with<T>(
        &self,
        op: impl AsyncFn(Arc<N>) -> Result<T, Error>,
        fails_over: fn(&Error) -> bool,
    ) -> Result<T, Error> {
        self.refresh_if_due().await;
        let (mut refreshed, mut moved, mut tried) = (false, false, 1);
        loop {
            let (_, node) = self.current();
            let token = self.session_token(&node).await;
            let e = match op(Arc::clone(&node)).await {
                Ok(v) => return Ok(v),
                Err(e) => e,
            };
            if wrong_node(&e) && !moved {
                moved = true;
                if self.move_on(&node).await {
                    tried += 1;
                    continue;
                }
            } else if fails_over(&e) && tried < self.len() {
                if self.move_on(&node).await {
                    tried += 1;
                    self.refresh_soon();
                    continue;
                }
            } else if unauthenticated(&e) && !refreshed {
                if let Some(token) = token {
                    refreshed = true;
                    self.renew(&token).await?;
                    continue;
                }
            }
            return Err(e);
        }
    }

    /// Run `op` once: a sign-in or sign-up. A `421 WRONG_NODE` still moves the
    /// client on, so the next attempt goes to the next node.
    pub(crate) async fn run_once<T>(
        &self,
        op: impl AsyncFnOnce(Arc<N>) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let (_, node) = self.current();
        let answer = op(Arc::clone(&node)).await;
        if matches!(&answer, Err(e) if wrong_node(e)) {
            let _moved = self.move_on(&node).await;
        }
        answer
    }

    /// A fresh session in place of the one `token` belonged to: the one a
    /// refresh by another request made meanwhile, or a refresh now.
    async fn renew(&self, token: &str) -> Result<(), Error> {
        let _one = self.refreshing.lock().await;
        // The node in use, not the one that refused: a refresh that moved on
        // left the new session on the node it moved to.
        let (_, node) = self.current();
        if self.session_token(&node).await.as_deref() != Some(token) {
            return Ok(());
        }
        self.exchange().await.map(drop)
    }

    /// Exchange the refresh token for a new session on the node in use, one at
    /// a time with every other refresh of this client.
    pub(crate) async fn refresh(&self) -> Result<Session, Error> {
        let _one = self.refreshing.lock().await;
        self.exchange().await
    }

    /// A node that may not serve the account answers `421 WRONG_NODE` before
    /// it spends the refresh token (core #4774), so the same token goes on to
    /// the next node (which [`move_on`](Self::move_on) hands the session),
    /// once per node at most. Every node refusing is the caller's `WrongNode`.
    async fn exchange(&self) -> Result<Session, Error> {
        let mut tried = 1;
        loop {
            let (_, node) = self.current();
            match node.refresh().await {
                Err(e) if wrong_node(&e) && tried < self.len() => {
                    tried += 1;
                    let _moved = self.move_on(&node).await;
                }
                answer => return answer,
            }
        }
    }

    /// Ask the directory again before the next request: a node failed.
    fn refresh_soon(&self) {
        if let Some(d) = &self.directory {
            if let Ok(mut due) = d.due.lock() {
                *due = Instant::now();
            }
        }
    }

    /// Learn the cluster's nodes again if a listing is due, best effort: a
    /// directory that cannot answer leaves the nodes as they are until the
    /// next is due. One request refreshes; the others go on meanwhile.
    async fn refresh_if_due(&self) {
        let Some(d) = &self.directory else { return };
        if d.due.lock().map_or(true, |due| Instant::now() < *due) {
            return;
        }
        let Ok(_one) = d.busy.try_lock() else { return };
        let (listed, after) = match d.source.fetch().await {
            Ok(listing) => listing,
            Err(_) => (Vec::new(), FAILED_REFRESH_AFTER),
        };
        if let Ok(mut due) = d.due.lock() {
            *due = Instant::now() + after;
        }
        if listed.is_empty() {
            return;
        }
        let (known, in_use) = {
            let s = self.read();
            (s.list.clone(), s.list[s.at].clone())
        };
        let list: Vec<(String, Arc<N>)> = merge(&in_use.0, &listed)
            .into_iter()
            .filter_map(|url| match known.iter().find(|(u, _)| *u == url) {
                Some((_, n)) => Some((url, Arc::clone(n))),
                None => d.source.connect(&url).ok().map(|n| (url, Arc::new(n))),
            })
            .collect();
        let mut s = self.write();
        // Unless a request moved on meanwhile: the next listing merges then.
        if Arc::ptr_eq(&s.list[s.at].1, &in_use.1) {
            *s = State { list, at: 0 };
        }
    }
}

/// How long a directory that could not answer is left before it is asked again.
const FAILED_REFRESH_AFTER: Duration = Duration::from_secs(60);

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Mutex;

    use serde_json::json;
    use tokio::sync::RwLock;

    use super::*;

    fn session(token: &str, refresh: &str) -> Session {
        Session {
            account_id: "acc-1".to_owned(),
            session_token: token.to_owned(),
            expires_at: 0,
            refresh_token: refresh.to_owned(),
        }
    }

    fn refusal(status: u16, code: &str) -> Error {
        Error::Api {
            status,
            code: code.to_owned(),
            message: format!("{} (stand-in)", code.to_lowercase()),
            details: json!({ "node": 1 }),
        }
    }

    /// A stand-in node: takes the session tokens in `valid`, exchanges
    /// `rt_old` for `v2.new` once (slowly, so refreshes can overlap), or
    /// answers every request with `refuse`.
    #[derive(Default)]
    struct Stand {
        session: RwLock<Option<Session>>,
        api_key: RwLock<Option<String>>,
        valid: Vec<&'static str>,
        refuse: Option<(u16, &'static str)>,
        refuse_refresh: Option<(u16, &'static str)>,
        refreshes: AtomicU32,
        requests: AtomicU32,
        /// The idempotency keys of the calls sent to this node.
        keys: Mutex<Vec<String>>,
    }

    impl Node for Stand {
        async fn session(&self) -> Option<Session> {
            self.session.read().await.clone()
        }
        async fn use_session(&self, s: Session) {
            *self.session.write().await = Some(s);
        }
        async fn use_api_key(&self, key: &str) {
            *self.api_key.write().await = Some(key.to_owned());
        }
        async fn refresh(&self) -> Result<Session, Error> {
            let _ = self.refreshes.fetch_add(1, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(20)).await;
            if let Some((status, code)) = self.refuse_refresh {
                return Err(refusal(status, code));
            }
            let current = self.session().await.expect("a session");
            if current.refresh_token != "rt_old" {
                return Err(refusal(401, "UNAUTHENTICATED"));
            }
            let new = session("v2.new", "rt_new");
            self.use_session(new.clone()).await;
            Ok(new)
        }
    }

    impl Stand {
        /// A request: answered by this node's own rules.
        async fn ask(&self) -> Result<&'static str, Error> {
            let _ = self.requests.fetch_add(1, Ordering::SeqCst);
            if let Some((status, code)) = self.refuse {
                return Err(refusal(status, code));
            }
            if self.api_key.read().await.is_some() {
                return Err(refusal(401, "UNAUTHENTICATED"));
            }
            let token = self.session().await.map(|s| s.session_token);
            match token {
                Some(t) if self.valid.contains(&t.as_str()) => Ok("v1"),
                _ => Err(refusal(401, "UNAUTHENTICATED")),
            }
        }
    }

    impl Stand {
        /// A call carrying `key`: recorded, then answered as [`ask`](Self::ask).
        async fn call(&self, key: &str) -> Result<&'static str, Error> {
            self.keys.lock().unwrap().push(key.to_owned());
            self.ask().await
        }
    }

    /// A node that answers every request `503`.
    fn unavailable_node() -> Stand {
        Stand {
            refuse: Some((503, "UNAVAILABLE")),
            ..Stand::default()
        }
    }

    /// A node that serves the session `v2.old`.
    fn serving() -> Stand {
        Stand {
            valid: vec!["v2.old"],
            ..Stand::default()
        }
    }

    impl<N: Node> Nodes<N> {
        fn node(&self, i: usize) -> Arc<N> {
            Arc::clone(&self.read().list[i].1)
        }
    }

    /// A stand-in directory: answers `listing` (or fails), counting fetches;
    /// a node it names that the client did not know is a new [`serving`] one.
    struct Listed {
        listing: Mutex<Result<Listing, String>>,
        fetches: Arc<AtomicU32>,
    }

    impl Directory<Stand> for Listed {
        fn fetch(&self) -> Pin<Box<dyn Future<Output = Result<Listing, String>> + Send + '_>> {
            let _ = self.fetches.fetch_add(1, Ordering::SeqCst);
            let answer = self.listing.lock().unwrap().clone();
            Box::pin(async move { answer })
        }
        fn connect(&self, _url: &str) -> Result<Stand, String> {
            Ok(serving())
        }
    }

    fn listed(urls: &[&str], after: Duration) -> (Box<Listed>, Arc<AtomicU32>) {
        let fetches = Arc::new(AtomicU32::new(0));
        let urls = urls.iter().map(|u| (*u).to_owned()).collect();
        let d = Listed {
            listing: Mutex::new(Ok((urls, after))),
            fetches: Arc::clone(&fetches),
        };
        (Box::new(d), fetches)
    }

    /// A node that can no longer vouch for `v2.old`'s issuer: it takes only `v2.new`.
    fn issuer_lost() -> Stand {
        Stand {
            valid: vec!["v2.new"],
            ..Stand::default()
        }
    }

    async fn signed_in(nodes: Vec<Stand>) -> Nodes<Stand> {
        let nodes = Nodes::new(
            nodes
                .into_iter()
                .enumerate()
                .map(|(i, n)| (format!("http://node-{i}"), n))
                .collect(),
        );
        nodes
            .current()
            .1
            .use_session(session("v2.old", "rt_old"))
            .await;
        nodes
    }

    #[tokio::test]
    async fn a_401_for_a_lost_issuer_refreshes_once_and_repeats_the_request() {
        let nodes = signed_in(vec![issuer_lost()]).await;
        let answer = nodes.run(async |n| n.ask().await).await;
        assert_eq!(answer.unwrap(), "v1");
        let (_, node) = nodes.current();
        assert_eq!(node.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(node.requests.load(Ordering::SeqCst), 2);
        assert_eq!(node.session().await.unwrap().session_token, "v2.new");
    }

    #[tokio::test]
    async fn a_refused_refresh_is_login_required_and_the_request_is_not_repeated() {
        let nodes = signed_in(vec![Stand {
            refuse_refresh: Some((401, "UNAUTHENTICATED")),
            ..issuer_lost()
        }])
        .await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert!(unauthenticated(&e), "{e}");
        let (_, node) = nodes.current();
        assert_eq!(node.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(node.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn requests_refused_at_the_same_time_share_one_refresh() {
        let nodes = Arc::new(signed_in(vec![issuer_lost()]).await);
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let nodes = Arc::clone(&nodes);
                tokio::spawn(async move { nodes.run(async |n| n.ask().await).await })
            })
            .collect();
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), "v1");
        }
        // A refresh token is single use: sent twice, the second would revoke the session.
        assert_eq!(nodes.current().1.refreshes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_second_401_after_the_refresh_is_final() {
        let nodes = signed_in(vec![Stand::default()]).await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert!(unauthenticated(&e), "{e}");
        let (_, node) = nodes.current();
        assert_eq!(node.refreshes.load(Ordering::SeqCst), 1);
        assert_eq!(node.requests.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_api_key_is_not_refreshed() {
        let nodes = signed_in(vec![issuer_lost()]).await;
        nodes.set_api_key(Some("cal_key".to_owned()));
        nodes.current().1.use_api_key("cal_key").await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert!(unauthenticated(&e), "{e}");
        assert_eq!(nodes.current().1.refreshes.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn wrong_node_moves_to_the_next_node_with_the_session_and_repeats_there() {
        let nodes = signed_in(vec![
            Stand {
                refuse: Some((421, "WRONG_NODE")),
                ..Stand::default()
            },
            Stand {
                valid: vec!["v2.old"],
                ..Stand::default()
            },
        ])
        .await;
        assert_eq!(nodes.run(async |n| n.ask().await).await.unwrap(), "v1");
        assert_eq!(nodes.url(), "http://node-1");
        assert_eq!(nodes.node(0).requests.load(Ordering::SeqCst), 1);
        assert_eq!(nodes.node(1).requests.load(Ordering::SeqCst), 1);
        // Later requests stay on the new node.
        assert_eq!(nodes.run(async |n| n.ask().await).await.unwrap(), "v1");
        assert_eq!(nodes.node(0).requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn wrong_node_with_no_other_node_is_the_callers() {
        let nodes = signed_in(vec![Stand {
            refuse: Some((421, "WRONG_NODE")),
            ..Stand::default()
        }])
        .await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert!(wrong_node(&e), "{e}");
        assert_eq!(e.details(), Some(&json!({ "node": 1 })));
        assert_eq!(nodes.current().1.requests.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn wrong_node_twice_is_the_callers_after_one_move() {
        let refusing = || Stand {
            refuse: Some((421, "WRONG_NODE")),
            ..Stand::default()
        };
        let nodes = signed_in(vec![refusing(), refusing(), refusing()]).await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert!(wrong_node(&e), "{e}");
        let asked: u32 = (0..3)
            .map(|i| nodes.node(i).requests.load(Ordering::SeqCst))
            .sum();
        assert_eq!(asked, 2);
    }

    #[tokio::test]
    async fn not_leader_is_not_a_move() {
        let nodes = signed_in(vec![
            Stand {
                refuse: Some((421, "NOT_LEADER")),
                ..Stand::default()
            },
            Stand::default(),
        ])
        .await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert_eq!(e.code(), Some("NOT_LEADER"));
        assert_eq!(nodes.url(), "http://node-0");
        assert_eq!(nodes.node(1).requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn a_sign_in_refused_wrong_node_is_not_repeated_but_the_next_goes_elsewhere() {
        let nodes = signed_in(vec![
            Stand {
                refuse: Some((421, "WRONG_NODE")),
                ..Stand::default()
            },
            Stand::default(),
        ])
        .await;
        let e = nodes.run_once(async |n| n.ask().await).await.unwrap_err();
        assert!(wrong_node(&e), "{e}");
        assert_eq!(nodes.node(1).requests.load(Ordering::SeqCst), 0);
        assert_eq!(nodes.url(), "http://node-1");
    }

    #[tokio::test]
    async fn a_refresh_refused_wrong_node_goes_to_the_next_node_with_the_same_token() {
        let nodes = signed_in(vec![
            Stand {
                refuse_refresh: Some((421, "WRONG_NODE")),
                ..issuer_lost()
            },
            issuer_lost(),
        ])
        .await;
        assert_eq!(nodes.run(async |n| n.ask().await).await.unwrap(), "v1");
        assert_eq!(nodes.url(), "http://node-1");
        assert_eq!(nodes.node(0).refreshes.load(Ordering::SeqCst), 1);
        // Node 1's stand-in exchanges only `rt_old`: the token was not spent.
        assert_eq!(nodes.node(1).refreshes.load(Ordering::SeqCst), 1);
        let (_, node) = nodes.current();
        assert_eq!(node.session().await.unwrap().session_token, "v2.new");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_requests_share_a_refresh_that_moved_on() {
        let nodes = Arc::new(
            signed_in(vec![
                Stand {
                    refuse_refresh: Some((421, "WRONG_NODE")),
                    ..issuer_lost()
                },
                issuer_lost(),
            ])
            .await,
        );
        let tasks: Vec<_> = (0..8)
            .map(|_| {
                let nodes = Arc::clone(&nodes);
                tokio::spawn(async move { nodes.run(async |n| n.ask().await).await })
            })
            .collect();
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), "v1");
        }
        assert_eq!(nodes.node(1).refreshes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_refresh_every_node_refuses_wrong_node_is_tried_once_per_node() {
        let refusing = || Stand {
            refuse_refresh: Some((421, "WRONG_NODE")),
            ..issuer_lost()
        };
        let nodes = signed_in(vec![refusing(), refusing(), refusing()]).await;
        let e = nodes.refresh().await.unwrap_err();
        assert!(wrong_node(&e), "{e}");
        for i in 0..3 {
            assert_eq!(nodes.node(i).refreshes.load(Ordering::SeqCst), 1);
        }
    }

    #[tokio::test]
    async fn an_unavailable_node_hands_a_call_to_the_next_with_the_same_key() {
        let nodes = signed_in(vec![unavailable_node(), serving()]).await;
        let key = "node:c-same";
        let answer = nodes.run_repeatable(async |n| n.call(key).await).await;
        assert_eq!(answer.unwrap(), "v1");
        assert_eq!(nodes.url(), "http://node-1");
        assert_eq!(*nodes.node(0).keys.lock().unwrap(), [key]);
        assert_eq!(*nodes.node(1).keys.lock().unwrap(), [key]);
        // The next node got the session to send it with.
        let s = nodes.node(1).session().await.unwrap();
        assert_eq!(s.session_token, "v2.old");
    }

    #[tokio::test]
    async fn every_node_unavailable_is_the_last_error_each_node_tried_once() {
        let nodes = signed_in(vec![
            unavailable_node(),
            Stand {
                refuse: Some((503, "LAST")),
                ..Stand::default()
            },
            unavailable_node(),
        ])
        .await;
        // Start at the second node: the try goes round to the first.
        let _moved = nodes.move_on(&nodes.node(0)).await;
        let e = nodes.run_repeatable(async |n| n.call("k").await).await;
        assert_eq!(e.unwrap_err().code(), Some("UNAVAILABLE"));
        for i in 0..3 {
            assert_eq!(nodes.node(i).requests.load(Ordering::SeqCst), 1, "node {i}");
        }
        assert_eq!(nodes.url(), "http://node-0");
    }

    #[tokio::test]
    async fn a_503_to_a_request_that_is_not_repeatable_stays_on_the_node() {
        let nodes = signed_in(vec![unavailable_node(), serving()]).await;
        let e = nodes.run(async |n| n.ask().await).await.unwrap_err();
        assert_eq!(e.code(), Some("UNAVAILABLE"));
        assert_eq!(nodes.url(), "http://node-0");
        assert_eq!(nodes.node(1).requests.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn an_unreachable_node_hands_any_request_to_the_next() {
        let refused = reqwest::Client::new()
            .get("http://127.0.0.1:9/")
            .send()
            .await
            .unwrap_err();
        let e = Error::Transport(eyre::Report::new(refused).wrap_err("attesting"));
        assert!(unreachable(&e) && unavailable(&e), "{e:#}");
        assert!(!unreachable(&refusal(503, "UNAVAILABLE")));
        assert!(!unreachable(&Error::Transport(eyre::eyre!("bad quote"))));

        let nodes = signed_in(vec![Stand::default(), serving()]).await;
        let first = nodes.node(0);
        let answer = nodes
            .run(async |n| {
                if Arc::ptr_eq(&n, &first) {
                    let refused = reqwest::Client::new().get("http://127.0.0.1:9/").send();
                    return Err(Error::Transport(refused.await.unwrap_err().into()));
                }
                n.ask().await
            })
            .await;
        assert_eq!(answer.unwrap(), "v1");
        assert_eq!(nodes.url(), "http://node-1");
    }

    #[tokio::test]
    async fn a_single_node_is_unchanged() {
        let nodes = signed_in(vec![unavailable_node()]).await;
        let e = nodes.run_repeatable(async |n| n.call("k").await).await;
        assert_eq!(e.unwrap_err().code(), Some("UNAVAILABLE"));
        assert_eq!(nodes.node(0).requests.load(Ordering::SeqCst), 1);
        assert_eq!(nodes.urls(), ["http://node-0"]);
    }

    #[tokio::test]
    async fn a_sign_in_is_not_sent_to_another_node_when_one_is_unavailable() {
        let nodes = signed_in(vec![unavailable_node(), serving()]).await;
        let e = nodes.run_once(async |n| n.ask().await).await.unwrap_err();
        assert_eq!(e.code(), Some("UNAVAILABLE"));
        assert_eq!(nodes.node(1).requests.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_merge_keeps_the_node_in_use_first_then_the_directorys_order() {
        let listed = ["http://c", "http://a", "http://d"].map(String::from);
        assert_eq!(
            merge("http://a", &listed),
            ["http://a", "http://c", "http://d"]
        );
        // No longer listed: kept only while in use.
        assert_eq!(
            merge("http://b", &listed),
            ["http://b", "http://c", "http://a", "http://d"]
        );
    }

    #[tokio::test]
    async fn a_failover_learns_the_nodes_again_and_merges_them() {
        let (dir, fetches) = listed(
            &["http://node-2", "http://new", "http://node-1"],
            Duration::from_secs(3600),
        );
        let nodes = signed_in(vec![unavailable_node(), serving(), unavailable_node()])
            .await
            .with_directory(dir, Duration::from_secs(3600));
        let kept = nodes.node(1);
        assert_eq!(
            nodes
                .run_repeatable(async |n| n.call("k").await)
                .await
                .unwrap(),
            "v1"
        );
        // Not due yet: nothing was fetched.
        assert_eq!(fetches.load(Ordering::SeqCst), 0);
        // The failover made a listing due before the next request.
        assert_eq!(
            nodes
                .run_repeatable(async |n| n.call("k2").await)
                .await
                .unwrap(),
            "v1"
        );
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(
            nodes.urls(),
            ["http://node-1", "http://node-2", "http://new"]
        );
        assert!(
            Arc::ptr_eq(&nodes.node(0), &kept),
            "the node in use is kept, session and all"
        );
        assert_eq!(nodes.url(), "http://node-1");
        // And not again until it is due.
        let _ = nodes.run_repeatable(async |n| n.call("k3").await).await;
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn the_nodes_are_learnt_again_once_the_listing_is_due() {
        let (dir, fetches) = listed(&["http://other"], Duration::ZERO);
        let nodes = signed_in(vec![serving(), serving()])
            .await
            .with_directory(dir, Duration::ZERO);
        assert_eq!(nodes.run(async |n| n.ask().await).await.unwrap(), "v1");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        // node-1 is no longer listed; node-0 is in use, so it stays, first.
        assert_eq!(nodes.urls(), ["http://node-0", "http://other"]);
    }

    #[tokio::test]
    async fn a_directory_that_cannot_answer_leaves_the_nodes_as_they_are() {
        let (dir, fetches) = listed(&[], Duration::ZERO);
        *dir.listing.lock().unwrap() = Err("directory down".to_owned());
        let nodes = signed_in(vec![serving(), serving()])
            .await
            .with_directory(dir, Duration::ZERO);
        assert_eq!(nodes.run(async |n| n.ask().await).await.unwrap(), "v1");
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
        assert_eq!(nodes.urls(), ["http://node-0", "http://node-1"]);
        // Not asked again on every request.
        let _ = nodes.run(async |n| n.ask().await).await;
        assert_eq!(fetches.load(Ordering::SeqCst), 1);
    }
}
