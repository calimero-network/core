//! Each app's replicas compare the digests of their applied state
//! (`calimero_log::digest`), so a replica that applied differently from the others
//! (a misread row, a skipped entry, a bug that is not deterministic) is noticed
//! instead of serving its own answers in silence.
//!
//! Every [`DIGEST_COMPARE_EVERY`], the leader of each app group asks each other
//! replica for its digests (`POST /internal/digests`) and compares them at the
//! latest index both recorded one. A difference is logged with the group, the
//! peer and the index, and shows in `calimero_group_diverged` until a later
//! comparison agrees. A peer on a release before the route answers `404` and is
//! not compared; a peer whose digests are of another `DIGEST_VERSION` neither.
//!
//! The system group is not compared: its machine keeps its state in memory and
//! writes no rows as it applies.

use std::collections::BTreeMap;
use std::time::Duration;

use calimero_log::{Comparison, Digests, GroupId, NodeId};
use serde::{Deserialize, Serialize};
use tracing::{debug, error, info};

use super::Node;

/// How often the leader of each app group compares its replicas' digests.
pub const DIGEST_COMPARE_EVERY: Duration = Duration::from_secs(60);

/// The most groups one `POST /internal/digests` names; a leader of more asks
/// again for the rest. Each group's answer is under 2 KiB.
pub const DIGEST_GROUPS_PER_ASK: usize = 256;

/// How long a leader waits for a peer's digests.
const DIGEST_WAIT: Duration = Duration::from_secs(10);

/// `POST /internal/digests`: the groups a peer asks the digests of.
#[derive(Debug, Serialize, Deserialize)]
pub struct DigestsAsk {
    pub groups: Vec<GroupId>,
}

/// The answer: this node's digests of each group it hosts and the caller
/// replicates too.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct DigestsAnswer {
    pub groups: BTreeMap<GroupId, Digests>,
}

/// One comparison of a group's digests with a replica's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Compared {
    pub group: GroupId,
    pub peer: NodeId,
    pub outcome: Comparison,
}

impl Node {
    /// This node's digests of `groups`, for `caller`: only of a group this node
    /// hosts and the caller is a member of, which holds the same state already.
    #[must_use]
    pub fn digests_for(&self, caller: NodeId, groups: &[GroupId]) -> DigestsAnswer {
        let mut out = DigestsAnswer::default();
        for group in groups {
            let Some(g) = self.app_group(*group) else {
                continue;
            };
            let status = g.status();
            if !status.voters.contains(&caller) && !status.learners.contains(&caller) {
                continue;
            }
            if let Some(d) = g.digests() {
                let _prev = out.groups.insert(*group, d);
            }
        }
        out
    }

    /// Compare the digests of every app group this node leads with each other
    /// replica's, recording what it finds ([`crate::metrics::digest_compared`]).
    pub async fn compare_digests(&self) -> Vec<Compared> {
        let me = self.config.node_id;
        // Each peer, and the groups led here it replicates: (group, our digests).
        let mut asks: BTreeMap<NodeId, Vec<(GroupId, Digests)>> = BTreeMap::new();
        let groups: Vec<_> = self
            .apps
            .read()
            .map(|a| {
                a.iter()
                    .map(|(id, g)| (*id, std::sync::Arc::clone(g)))
                    .collect()
            })
            .unwrap_or_default();
        for (id, g) in groups {
            let status = g.status();
            if !status.is_leader {
                continue;
            }
            let Some(ours) = g.digests() else { continue };
            for peer in status.voters.iter().chain(&status.learners) {
                if *peer != me {
                    asks.entry(*peer).or_default().push((id, ours.clone()));
                }
            }
        }
        let rounds = asks.into_iter().map(|(peer, groups)| async move {
            let mut out = Vec::new();
            let Some(addr) = self.sys(|s| s.nodes.get(&peer).map(|n| n.addr.clone())) else {
                return out;
            };
            for chunk in groups.chunks(DIGEST_GROUPS_PER_ASK) {
                let ask = DigestsAsk {
                    groups: chunk.iter().map(|(g, _)| *g).collect(),
                };
                let answer = match tokio::time::timeout(
                    DIGEST_WAIT,
                    self.peer_post(&addr, "internal/digests", &ask),
                )
                .await
                {
                    Ok(Ok(v)) => serde_json::from_value::<DigestsAnswer>(v).ok(),
                    Ok(Err(e)) => {
                        // A peer on a release before the route answers 404.
                        debug!(peer, error = %e, "a replica did not answer its digests");
                        None
                    }
                    Err(_) => {
                        debug!(peer, "a replica did not answer its digests in time");
                        None
                    }
                };
                let Some(answer) = answer else { continue };
                for (group, ours) in chunk {
                    let Some(theirs) = answer.groups.get(group) else {
                        continue;
                    };
                    out.push(Compared {
                        group: *group,
                        peer,
                        outcome: ours.compare(theirs),
                    });
                }
            }
            out
        });
        let compared: Vec<Compared> = futures_util::future::join_all(rounds)
            .await
            .into_iter()
            .flatten()
            .collect();
        for c in &compared {
            let changed = crate::metrics::digest_compared(c.group, c.peer, c.outcome);
            match c.outcome {
                Comparison::Differ {
                    index,
                    ours,
                    theirs,
                } if changed => {
                    error!(group = c.group, peer = c.peer, index, %ours, %theirs,
                        "a replica's state differs from this leader's at the same index: \
                         it applied differently (see the runbook, \"A replica's state differs\")");
                }
                Comparison::Agree { index } if changed => {
                    info!(
                        group = c.group,
                        peer = c.peer,
                        index,
                        "a replica's state agrees with this leader's again"
                    );
                }
                _ => {}
            }
        }
        compared
    }
}
