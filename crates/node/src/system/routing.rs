//! What a Raft message between nodes asks of the system state, answered
//! without scanning every app: the app a group replicates, the apps a node
//! hosts, and whether a release may hold a group's data.
//!
//! Derived from the applied state, never stored or replicated: built on the
//! first question after an entry applies or a snapshot restores, and dropped
//! by the next ([`super::SystemMachine::forget_routing`]). Every app group's
//! heartbeat asks it (`Node::may_send` going out, `internal_raft` coming in),
//! so a lookup is a hash probe and clones no record; the system state changes
//! far less often, and one rebuild (O(apps)) serves every message until then.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use super::SystemState;
use crate::types::{AppId, AppRecord};

/// The indexes and the decisions cached since the system state last changed.
#[derive(Default)]
pub(super) struct Routing {
    index: OnceLock<Index>,
    /// `(group, release)` -> [`SystemState::release_may_hold`] for the group's
    /// app. Releases are few, so this stays near groups x releases.
    approvals: Mutex<HashMap<(u64, Option<String>), bool>>,
    /// Indexes built since this machine started (tests read it).
    #[cfg(test)]
    pub(super) builds: std::sync::atomic::AtomicU64,
}

#[derive(Default)]
struct Index {
    /// Group -> the app it replicates (the first by app id, as a scan finds it).
    by_group: HashMap<u64, AppId>,
    /// Node -> the apps that name it a replica, by app id.
    by_replica: HashMap<u64, Vec<AppId>>,
}

impl Index {
    fn build(s: &SystemState) -> Self {
        let mut ix = Self::default();
        for (id, app) in &s.apps {
            let _first = ix.by_group.entry(app.group).or_insert_with(|| id.clone());
            for node in &app.replicas {
                ix.by_replica.entry(*node).or_default().push(id.clone());
            }
        }
        ix
    }
}

impl Routing {
    fn index(&self, s: &SystemState) -> &Index {
        self.index.get_or_init(|| {
            #[cfg(test)]
            let _n = self
                .builds
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            Index::build(s)
        })
    }

    /// The app group `group` replicates.
    pub(super) fn app_of_group<'s>(&self, s: &'s SystemState, group: u64) -> Option<&'s AppRecord> {
        s.apps.get(self.index(s).by_group.get(&group)?)
    }

    /// The apps that name `node` a replica, by app id.
    pub(super) fn apps_on<'s>(
        &'s self,
        s: &'s SystemState,
        node: u64,
    ) -> impl Iterator<Item = &'s AppRecord> + 's {
        self.index(s)
            .by_replica
            .get(&node)
            .into_iter()
            .flatten()
            .filter_map(|id| s.apps.get(id))
    }

    /// Whether a node on `release` may hold the data of the app `group`
    /// replicates; `None` for a group no app has.
    pub(super) fn group_may_hold(
        &self,
        s: &SystemState,
        group: u64,
        release: Option<&str>,
    ) -> Option<bool> {
        let app = self.app_of_group(s, group)?;
        let mut approvals = self
            .approvals
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let key = (group, release.map(str::to_owned));
        if let Some(v) = approvals.get(&key) {
            return Some(*v);
        }
        let v = s.release_may_hold(app, release);
        let _prev = approvals.insert(key, v);
        Some(v)
    }

    /// Drop the index and the decisions: the state they were derived from changed.
    pub(super) fn forget(&mut self) {
        self.index = OnceLock::new();
        self.approvals
            .get_mut()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    use calimero_log::Machine as _;
    use serde_json::json;

    use super::super::{SystemCmd, SystemMachine, SystemState};
    use crate::types::AppRecord;

    fn app(i: u64) -> AppRecord {
        serde_json::from_value(json!({
            "appId": format!("app-{i:06}"), "name": "a", "group": i + 2, "bundle": "b",
            "owner": "alice", "org": "org", "replicas": [i % 5 + 1, (i + 1) % 5 + 1],
            "createdAt": 10,
        }))
        .unwrap()
    }

    /// A machine restored from a state holding `n` apps, as a node's system
    /// group is after a restart or a snapshot.
    fn machine(n: u64) -> SystemMachine {
        let mut state = SystemState::default();
        for i in 0..n {
            let a = app(i);
            let _prev = state.apps.insert(a.app_id.clone(), a);
        }
        let mut m = SystemMachine::default();
        m.restore(&serde_json::to_vec(&state).unwrap()).unwrap();
        m
    }

    /// `rounds` passes over every group, as `may_send` and `internal_raft`
    /// ask: the app and the decision for a peer's release.
    fn route_all(m: &SystemMachine, n: u64, rounds: u64) -> Duration {
        let start = Instant::now();
        for _ in 0..rounds {
            for i in 0..n {
                let group = i + 2;
                let held = m.app_of_group(group).map(|a| a.replicas.contains(&1));
                let ok = m.group_may_hold(group, Some("1.0.0"));
                assert_eq!(held.is_some(), ok.is_some());
            }
        }
        start.elapsed()
    }

    /// A Raft message costs the same at 10,000 apps as at 100: routing it is
    /// an index probe and a cached decision, not a scan of the apps.
    /// A scaling guard (docs/benchmarking.md): the same 100,000 messages, a
    /// 100x larger directory; it was ~100x slower with the scan.
    #[test]
    fn routing_a_raft_message_does_not_scan_the_apps() {
        const MESSAGES: u64 = 100_000;
        let small = machine(100);
        let large = machine(10_000);
        // Warm both (the one rebuild after a restore is O(apps) and not
        // what a message pays), then take the best of three.
        let _ = route_all(&small, 100, 1);
        let _ = route_all(&large, 10_000, 1);
        let best = |m: &SystemMachine, n: u64| {
            (0..3).map(|_| route_all(m, n, MESSAGES / n)).min().unwrap()
        };
        let (t_small, t_large) = (best(&small, 100), best(&large, 10_000));
        let ratio = t_large.as_secs_f64() / t_small.as_secs_f64();
        println!(
            "{MESSAGES} messages: 100 apps {t_small:?}, 10000 apps {t_large:?}, ratio {ratio:.1}"
        );
        assert!(ratio < 10.0, "routing grows with the apps: x{ratio:.1}");
        // Each machine built its index once, for every message since.
        assert_eq!(small.routing.builds.load(Ordering::Relaxed), 1);
        assert_eq!(large.routing.builds.load(Ordering::Relaxed), 1);
    }

    /// The index and the decisions go with the state they came from: an
    /// applied entry or a restore is seen by the next message.
    #[test]
    fn an_applied_entry_or_a_restore_is_seen_by_the_next_message() {
        let mut m = machine(10);
        assert_eq!(m.app_of_group(2).unwrap().app_id, "app-000000");
        assert_eq!(m.apps_on(1).count(), 4);
        assert_eq!(m.group_may_hold(2, None), Some(true));
        assert_eq!(m.group_may_hold(99, None), None);

        // What an entry does to the directory, then the entry applied.
        let moved = m.state.apps.get_mut("app-000000").unwrap();
        moved.group = 99;
        moved.replicas = vec![7];
        let dir = tempfile::tempdir().unwrap();
        let engine = calimero_log::Engine::open(
            1,
            dir.path(),
            &[7; 32],
            std::sync::Arc::new(NoPeers),
            calimero_log::Config::default(),
        )
        .unwrap();
        let group = engine.group_state(1);
        let mut tx = calimero_log::StateTx::new(&group);
        let entry = serde_json::to_vec(&SystemCmd::RequireSessionProofs).unwrap();
        let _out = m.apply(5, &entry, &mut tx);
        assert!(m.app_of_group(2).is_none());
        assert_eq!(m.app_of_group(99).unwrap().app_id, "app-000000");
        assert_eq!(m.apps_on(7).count(), 1);
        assert_eq!(m.apps_on(1).count(), 3);
        assert_eq!(m.group_may_hold(2, None), None);
        assert_eq!(m.group_may_hold(99, None), Some(true));
        assert_eq!(m.routing.builds.load(Ordering::Relaxed), 2);

        // A snapshot restored over it.
        m.restore(&serde_json::to_vec(&machine(3).state).unwrap())
            .unwrap();
        assert_eq!(m.app_of_group(2).unwrap().app_id, "app-000000");
        assert!(m.app_of_group(99).is_none());
        assert!(m.app_of_group(5).is_none());
    }

    /// A release the app's organization does not approve stays refused
    /// from the cache, and an approval applied later is seen.
    #[test]
    fn a_cached_refusal_ends_with_the_approval_that_lifts_it() {
        let mut m = machine(1);
        let org: crate::types::Org = serde_json::from_value(json!({
            "orgId": "org", "name": "org", "members": {}, "teams": {},
            "approvedReleases": ["1.0.0"],
        }))
        .unwrap();
        let _prev = m.state.orgs.insert("org".into(), org);
        m.forget_routing();
        assert_eq!(m.group_may_hold(2, Some("1.0.0")), Some(true));
        assert_eq!(m.group_may_hold(2, Some("2.0.0")), Some(false));
        assert_eq!(m.group_may_hold(2, None), Some(false));
        let _new = m
            .state
            .orgs
            .get_mut("org")
            .unwrap()
            .approved_releases
            .insert("2.0.0".into());
        // Cached until the state changes through the log...
        assert_eq!(m.group_may_hold(2, Some("2.0.0")), Some(false));
        m.forget_routing();
        // ...and seen once it has.
        assert_eq!(m.group_may_hold(2, Some("2.0.0")), Some(true));
    }

    struct NoPeers;

    impl calimero_log::Transport for NoPeers {
        fn send(
            &self,
            _target: calimero_log::NodeId,
            _addr: String,
            _group: calimero_log::GroupId,
            _request: calimero_log::RpcRequest,
        ) -> calimero_log::BoxFuture<Result<calimero_log::RpcResponse, String>> {
            Box::pin(async { Err("no peers".to_owned()) })
        }
    }
}
