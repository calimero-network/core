//! Refusing a store older than mero-kms has seen (`docs/specs/rollback-protection.md`).
//!
//! mero-kms keeps, per node of a cluster, an entry `(generation, mark)`, and a
//! cluster epoch (the number of rollbacks an operator forced). The node keeps
//! the same entry in its sealed store (`Engine::put_node_meta`, row
//! [`ROW`]), bound to its node id, and raises both every [`CHECKPOINT_EVERY`]:
//! its store first, then mero-kms. A host that restarts the node on an older
//! copy of its disk hands it a store whose entry is below mero-kms's, and
//! [`start`] refuses it. `--accept-rollback` starts it anyway: mero-kms lowers
//! the node's entry and raises the cluster epoch, which the node reports.

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use calimero_log::{Engine, NodeId};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// The store row that holds the node's entry.
pub const ROW: &str = "rollback";

/// How often a running node raises its entry: a host that rolls a node back
/// by less than this is not caught (Raft still holds those writes elsewhere).
pub const CHECKPOINT_EVERY: Duration = Duration::from_secs(1);

/// A node's entry: compared as a pair, so a forced rollback (a new generation)
/// is larger than any mark of the generation before.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct Entry {
    pub generation: u64,
    pub mark: u64,
}

impl Entry {
    /// The next checkpoint after `self`.
    #[must_use]
    pub const fn next(self) -> Self {
        Self {
            generation: self.generation,
            mark: self.mark.saturating_add(1),
        }
    }
}

/// What mero-kms said with the key: the cluster epoch and this node's entry
/// (`None`: it has none, as for a node it never saw).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grant {
    pub epoch: u64,
    pub entry: Option<Entry>,
}

/// mero-kms's answer to a raise or a reset: the cluster epoch and the entry it
/// now holds for this node.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ack {
    pub epoch: u64,
    pub entry: Entry,
}

/// What the node's store holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Record {
    pub node: NodeId,
    pub entry: Entry,
    pub epoch: u64,
}

pub type CounterFuture<'a> = Pin<Box<dyn Future<Output = eyre::Result<Ack>> + Send + 'a>>;

/// Where the counter lives: mero-kms (`crate::kms::KmsCounter`), or memory in tests.
pub trait Counter: Debug + Send + Sync {
    /// What the key release said. `None`: the KMS keeps no counters (an older
    /// mero-kms), so the store cannot be judged.
    fn grant(&self) -> Option<Grant>;
    /// Raise this node's entry to `to` (never lowers it).
    fn raise(&self, to: Entry) -> CounterFuture<'_>;
    /// Force this node's entry down to `mark` in a new generation, raising the
    /// cluster epoch (`--accept-rollback`).
    fn reset(&self, mark: u64) -> CounterFuture<'_>;
}

/// What [`judge`] decides.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Start, checkpointing from this entry.
    Start(Entry),
    /// The KMS keeps no counters and the store never had one: start, unprotected.
    Unprotected,
    /// The store is behind (or not this node's); `--accept-rollback` may start it.
    RolledBack(String),
    /// Never started, whatever the flags.
    Refused(String),
}

/// Judge the store's `record` against mero-kms's `grant` for node `node`.
#[must_use]
pub fn judge(node: NodeId, grant: Option<Grant>, record: Option<Record>) -> Verdict {
    if let Some(r) = record {
        if r.node != node {
            return Verdict::Refused(format!(
                "this store belongs to node {}, not node {node}: a store does not move to \
                 another node id",
                r.node
            ));
        }
    }
    let Some(grant) = grant else {
        return match record {
            None => Verdict::Unprotected,
            Some(_) => Verdict::Refused(
                "this store has a rollback counter, but mero-kms answered without one (an \
                 older mero-kms, or a request stripped in transit): refusing to run unprotected"
                    .to_owned(),
            ),
        };
    };
    match (grant.entry, record) {
        (None, None) => Verdict::Start(Entry {
            generation: grant.epoch,
            mark: 0,
        }),
        (Some(kms), None) => Verdict::RolledBack(format!(
            "mero-kms has seen node {node} at checkpoint {} (generation {}), and this store has \
             no checkpoint: it is empty, or older than rollback protection",
            kms.mark, kms.generation
        )),
        (None, Some(r)) => Verdict::RolledBack(format!(
            "this store is at checkpoint {} and mero-kms has no record of node {node}: \
             mero-kms lost its counters",
            r.entry.mark
        )),
        (Some(kms), Some(r)) if r.entry < kms => Verdict::RolledBack(format!(
            "this store is at checkpoint {} (generation {}) and mero-kms has seen node {node} \
             at checkpoint {} (generation {}): the store was rolled back",
            r.entry.mark, r.entry.generation, kms.mark, kms.generation
        )),
        (Some(kms), Some(r)) => Verdict::Start(kms.max(r.entry)),
    }
}

/// What the node reports of its counter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// The cluster epoch: rollbacks forced so far.
    pub epoch: u64,
    pub generation: u64,
    pub mark: u64,
    /// Whether this start was forced (`--accept-rollback`).
    pub forced: bool,
    /// Seconds since mero-kms last acknowledged a checkpoint.
    pub since_checkpoint_secs: u64,
}

/// A started node's counter: its state, and the loop that raises it.
pub struct Checkpoints {
    engine: Arc<Engine>,
    node: NodeId,
    counter: Arc<dyn Counter>,
    state: Mutex<State>,
}

#[derive(Debug, Clone, Copy)]
struct State {
    epoch: u64,
    entry: Entry,
    forced: bool,
    acked_at: Instant,
}

/// Judge the store and, if it may start, write and acknowledge its first
/// checkpoint. An error means the node must not start.
pub async fn start(
    engine: &Arc<Engine>,
    node: NodeId,
    counter: Arc<dyn Counter>,
    accept_rollback: bool,
) -> eyre::Result<Option<Arc<Checkpoints>>> {
    let record: Option<Record> = engine.get_node_meta(ROW)?;
    let grant = counter.grant();
    let (from, epoch, forced) = match judge(node, grant, record) {
        Verdict::Start(entry) => (entry, grant.map_or(0, |g| g.epoch), false),
        Verdict::Unprotected => {
            warn!(
                "mero-kms keeps no rollback counters: this node starts UNPROTECTED against a \
                 restored older copy of its disk"
            );
            return Ok(None);
        }
        Verdict::Refused(why) => eyre::bail!("refusing to start: {why}"),
        Verdict::RolledBack(why) if !accept_rollback => eyre::bail!(
            "refusing to start: {why}. If the cluster's newest disks are lost, start every node \
             with --accept-rollback (mero-kms then raises the cluster's epoch, which every node \
             reports); if only this node is behind, wipe it and register it under a new node id"
        ),
        Verdict::RolledBack(why) => {
            let mark = record.map_or(0, |r| r.entry.mark);
            let ack = counter.reset(mark).await?;
            warn!(
                epoch = ack.epoch,
                mark, "--accept-rollback: starting on a store mero-kms had seen newer ({why})"
            );
            (ack.entry, ack.epoch, true)
        }
    };
    let checkpoints = Arc::new(Checkpoints {
        engine: Arc::clone(engine),
        node,
        counter,
        state: Mutex::new(State {
            epoch,
            entry: from,
            forced,
            acked_at: Instant::now(),
        }),
    });
    checkpoints.checkpoint().await?;
    info!(
        epoch,
        generation = from.generation,
        mark = from.mark,
        "rollback counter checked"
    );
    Ok(Some(checkpoints))
}

impl Debug for Checkpoints {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Checkpoints")
            .field("node", &self.node)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

impl Checkpoints {
    /// Raise the entry by one: the store first (synced), then mero-kms, so a
    /// crash between the two leaves the store ahead, which starts.
    pub async fn checkpoint(&self) -> eyre::Result<()> {
        let (next, epoch) = {
            let s = self.state.lock().map_err(|e| eyre::eyre!("{e}"))?;
            (s.entry.next(), s.epoch)
        };
        self.engine.put_node_meta(
            ROW,
            &Record {
                node: self.node,
                entry: next,
                epoch,
            },
        )?;
        let ack = self.counter.raise(next).await?;
        let mut s = self.state.lock().map_err(|e| eyre::eyre!("{e}"))?;
        s.entry = s.entry.max(next);
        s.epoch = s.epoch.max(ack.epoch);
        s.acked_at = Instant::now();
        Ok(())
    }

    /// Checkpoint every [`CHECKPOINT_EVERY`] until `stop`. A failed raise is
    /// logged and retried: the node keeps serving (see the design's open
    /// question 2), and [`Checkpoints::status`] shows how stale the counter is.
    pub async fn run(self: Arc<Self>, stop: CancellationToken) {
        loop {
            tokio::select! {
                () = stop.cancelled() => return,
                () = tokio::time::sleep(CHECKPOINT_EVERY) => {}
            }
            if let Err(e) = self.checkpoint().await {
                warn!("rollback checkpoint failed: {e:#}");
            }
        }
    }

    #[must_use]
    pub fn status(&self) -> Option<Status> {
        let s = *self.state.lock().ok()?;
        Some(Status {
            epoch: s.epoch,
            generation: s.entry.generation,
            mark: s.entry.mark,
            forced: s.forced,
            since_checkpoint_secs: s.acked_at.elapsed().as_secs(),
        })
    }
}

/// A counter in memory, standing in for mero-kms in tests: it outlives the
/// nodes started against it, as mero-kms outlives a node's restarts.
#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
pub struct MemoryCounter {
    inner: Mutex<MemoryInner>,
}

#[cfg(any(test, feature = "testing"))]
#[derive(Debug, Default)]
struct MemoryInner {
    epoch: u64,
    entry: Option<Entry>,
    /// Answer as a mero-kms without counters.
    absent: bool,
}

#[cfg(any(test, feature = "testing"))]
impl MemoryCounter {
    /// The entry mero-kms holds for the node, and the cluster epoch.
    #[must_use]
    pub fn seen(&self) -> (u64, Option<Entry>) {
        self.inner
            .lock()
            .map(|i| (i.epoch, i.entry))
            .unwrap_or_default()
    }

    /// Answer from now on as a mero-kms that keeps no counters.
    pub fn forget_counters(&self) {
        if let Ok(mut i) = self.inner.lock() {
            i.absent = true;
        }
    }
}

#[cfg(any(test, feature = "testing"))]
impl Counter for MemoryCounter {
    fn grant(&self) -> Option<Grant> {
        let i = self.inner.lock().ok()?;
        (!i.absent).then_some(Grant {
            epoch: i.epoch,
            entry: i.entry,
        })
    }

    fn raise(&self, to: Entry) -> CounterFuture<'_> {
        Box::pin(async move {
            let mut i = self.inner.lock().map_err(|e| eyre::eyre!("{e}"))?;
            let entry = i.entry.map_or(to, |e| e.max(to));
            i.entry = Some(entry);
            Ok(Ack {
                epoch: i.epoch,
                entry,
            })
        })
    }

    fn reset(&self, mark: u64) -> CounterFuture<'_> {
        Box::pin(async move {
            let mut i = self.inner.lock().map_err(|e| eyre::eyre!("{e}"))?;
            i.epoch += 1;
            let entry = Entry {
                generation: i.epoch,
                mark,
            };
            i.entry = Some(entry);
            Ok(Ack {
                epoch: i.epoch,
                entry,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const E: fn(u64, u64) -> Entry = |generation, mark| Entry { generation, mark };

    fn rec(node: NodeId, generation: u64, mark: u64) -> Option<Record> {
        Some(Record {
            node,
            entry: E(generation, mark),
            epoch: generation,
        })
    }

    fn grant(epoch: u64, entry: Option<Entry>) -> Option<Grant> {
        Some(Grant { epoch, entry })
    }

    #[test]
    fn a_store_at_or_past_the_kms_starts() {
        assert_eq!(
            judge(1, grant(0, Some(E(0, 5))), rec(1, 0, 5)),
            Verdict::Start(E(0, 5))
        );
        assert_eq!(
            judge(1, grant(0, Some(E(0, 5))), rec(1, 0, 6)),
            Verdict::Start(E(0, 6))
        );
        assert_eq!(judge(1, grant(3, None), None), Verdict::Start(E(3, 0)));
    }

    #[test]
    fn a_store_behind_the_kms_is_rolled_back() {
        for (g, r) in [
            (grant(0, Some(E(0, 5))), rec(1, 0, 4)),
            (grant(1, Some(E(1, 0))), rec(1, 0, 99)),
            (grant(0, Some(E(0, 5))), None),
            (grant(0, None), rec(1, 0, 4)),
        ] {
            assert!(
                matches!(judge(1, g, r), Verdict::RolledBack(_)),
                "{g:?} {r:?}"
            );
        }
    }

    #[test]
    fn another_nodes_store_and_a_stripped_answer_are_refused() {
        assert!(matches!(
            judge(2, grant(0, Some(E(0, 5))), rec(1, 0, 9)),
            Verdict::Refused(_)
        ));
        assert!(matches!(judge(1, None, rec(1, 0, 9)), Verdict::Refused(_)));
        assert_eq!(judge(1, None, None), Verdict::Unprotected);
    }
}
