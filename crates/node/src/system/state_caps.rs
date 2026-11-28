//! Bounds on what one organization and one account keep in the replicated system state: teams
//! per organization, and the metadata bytes of an organization and of the organizations an
//! account owns. Every node holds the system state in memory and writes all of it into each
//! snapshot, so each bound is a quota checked at apply and before a write is proposed.
//!
//! Spec: [Limits](https://calimero-network.github.io/core/protocol/limits/#quotas)

use std::io::Write;

use serde_json::Value;

use super::cluster_version::Feature;
use super::{Rejection, SystemCmd, SystemState};
use crate::types::{Org, OrgRole};

/// Teams one organization holds, app teams included (`Quotas::max_teams_per_org`).
pub const MAX_TEAMS_PER_ORG: u32 = 1_000;
/// Metadata bytes one organization holds: its own, its members' and its teams', as JSON
/// (`Quotas::max_metadata_bytes_per_org`).
pub const MAX_METADATA_BYTES_PER_ORG: u64 = 1024 * 1024;
/// Metadata bytes of every organization one account owns, summed
/// (`Quotas::max_metadata_bytes_per_account`).
pub const MAX_METADATA_BYTES_PER_ACCOUNT: u64 = 2 * 1024 * 1024;

/// Where a metadata write lands in its organization.
#[derive(Clone, Copy, Debug)]
pub enum MetadataSlot<'a> {
    Org,
    Member(&'a str),
    Team(&'a str),
}

impl SystemState {
    /// Whether the logged cluster version brings these bounds to apply
    /// ([`Feature::StateCaps`]): an entry before it replays as it applied.
    pub(super) fn caps_state(&self) -> bool {
        self.cluster_brings(Feature::StateCaps)
    }

    /// The bounds `cmd` would cross, judged before it is proposed: a new team past
    /// `maxTeamsPerOrg`, or metadata past `maxMetadataBytesPerOrg` or
    /// `maxMetadataBytesPerAccount`. Every other command passes.
    ///
    /// # Errors
    /// `429 QUOTA_EXCEEDED`, as apply would refuse it.
    pub fn check_state_caps(&self, cmd: &SystemCmd) -> Result<(), Rejection> {
        match cmd {
            SystemCmd::CreateTeam { org, .. } => self.check_new_team(org),
            SystemCmd::SetOrgMetadata { org, metadata, .. } => {
                self.check_metadata(org, MetadataSlot::Org, metadata)
            }
            SystemCmd::SetMemberMetadata {
                org,
                account,
                metadata,
                ..
            } => self.check_metadata(org, MetadataSlot::Member(account), metadata),
            SystemCmd::UpdateTeam {
                org,
                team,
                metadata: Some(metadata),
                ..
            } => self.check_metadata(org, MetadataSlot::Team(team), metadata),
            _ => Ok(()),
        }
    }

    /// One more team fits in `org` (`maxTeamsPerOrg`). An organization that does not exist
    /// passes: the command is refused for that.
    ///
    /// # Errors
    /// `429 QUOTA_EXCEEDED` when the organization holds the most teams it may.
    pub(super) fn check_new_team(&self, org: &str) -> Result<(), Rejection> {
        let Some(organization) = self.orgs.get(org) else {
            return Ok(());
        };
        if organization.teams.len() >= self.quotas.max_teams_per_org as usize {
            return Err(Rejection::quota("teams in this organization"));
        }
        Ok(())
    }

    /// `metadata` written at `slot` keeps `org` within `maxMetadataBytesPerOrg` and its owner
    /// within `maxMetadataBytesPerAccount`. A write that does not grow a total always passes,
    /// so an organization over a bound (from before it, or a lowered quota) can still shrink.
    ///
    /// # Errors
    /// `429 QUOTA_EXCEEDED` when the write grows a total past its bound.
    pub(super) fn check_metadata(
        &self,
        org: &str,
        slot: MetadataSlot<'_>,
        metadata: &Value,
    ) -> Result<(), Rejection> {
        let Some(organization) = self.orgs.get(org) else {
            return Ok(());
        };
        let old = slot_bytes(organization, slot);
        let new = json_bytes(metadata);
        if new <= old {
            return Ok(());
        }
        let grows = new - old;
        let in_org = org_metadata_bytes(organization);
        if in_org.saturating_add(grows) > self.quotas.max_metadata_bytes_per_org {
            return Err(Rejection::quota("metadata bytes in this organization"));
        }
        let Some(owner) = owner_of(organization) else {
            return Ok(());
        };
        let owned: u64 = self
            .orgs
            .values()
            .filter(|o| o.role(owner) == Some(OrgRole::Owner))
            .map(org_metadata_bytes)
            .sum();
        if owned.saturating_add(grows) > self.quotas.max_metadata_bytes_per_account {
            return Err(Rejection::quota(
                "metadata bytes in the organizations its owner owns",
            ));
        }
        Ok(())
    }
}

/// The organization's owner.
fn owner_of(org: &Org) -> Option<&str> {
    org.members
        .iter()
        .find(|(_, role)| **role == OrgRole::Owner)
        .map(|(account, _)| account.as_str())
}

/// The metadata bytes at `slot` now: 0 where none is set.
fn slot_bytes(org: &Org, slot: MetadataSlot<'_>) -> u64 {
    match slot {
        MetadataSlot::Org => json_bytes(&org.metadata),
        MetadataSlot::Member(account) => org.member_metadata.get(account).map_or(0, json_bytes),
        MetadataSlot::Team(team) => org.teams.get(team).map_or(0, |t| json_bytes(&t.metadata)),
    }
}

/// The metadata bytes an organization holds: its own, its members' and its teams'.
#[must_use]
pub fn org_metadata_bytes(org: &Org) -> u64 {
    json_bytes(&org.metadata)
        + org.member_metadata.values().map(json_bytes).sum::<u64>()
        + org
            .teams
            .values()
            .map(|t| json_bytes(&t.metadata))
            .sum::<u64>()
}

/// A metadata value's size as JSON; `null` (no metadata) is 0.
fn json_bytes(v: &Value) -> u64 {
    if v.is_null() {
        return 0;
    }
    let mut count = Count(0);
    // SAFETY: writing a `Value` into a sink that never fails cannot fail.
    serde_json::to_writer(&mut count, v).expect("a Value serializes");
    count.0
}

/// A writer that only counts the bytes it is given.
struct Count(u64);

impl Write for Count {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0 += buf.len() as u64;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::orgs::new_org;
    use super::super::{RejectionKind, SystemMachine};
    use super::*;
    use crate::types::TeamJoin;

    /// A machine at cluster `version` where `alice` owns `org` and `org2`, with bounds small
    /// enough to reach.
    fn machine(version: &str) -> SystemMachine {
        let mut m = SystemMachine::new("c".into(), crate::system::shard::ShardId::Root);
        m.state.cluster_version = Some(version.to_owned());
        for org in ["org", "org2"] {
            let o = new_org(org.into(), org.into(), "alice".into(), None, false);
            let _prev = m.state.orgs.insert(org.into(), o);
        }
        m.state.quotas.max_teams_per_org = 2;
        m.state.quotas.max_metadata_bytes_per_org = 100;
        m.state.quotas.max_metadata_bytes_per_account = 150;
        m
    }

    fn team(org: &str, id: &str) -> SystemCmd {
        SystemCmd::CreateTeam {
            org: org.into(),
            team_id: id.into(),
            name: id.into(),
            parent: None,
            join: TeamJoin::Invite,
            actor: "alice".into(),
            member_child_teams: true,
        }
    }

    fn org_metadata(org: &str, bytes: usize) -> SystemCmd {
        SystemCmd::SetOrgMetadata {
            org: org.into(),
            // `"…"`: the quotes are two of its bytes.
            metadata: json!("x".repeat(bytes - 2)),
            actor: "alice".into(),
        }
    }

    fn refused_as_quota(m: &mut SystemMachine, cmd: SystemCmd) {
        assert_eq!(
            m.state.check_state_caps(&cmd).map_err(|r| r.kind()),
            Err(RejectionKind::QuotaExceeded)
        );
        let refused = m.run_authorized(cmd).unwrap_err();
        assert_eq!(refused.kind(), RejectionKind::QuotaExceeded, "{refused:?}");
    }

    /// From the version that brings them, apply refuses a team past `maxTeamsPerOrg` and
    /// metadata past `maxMetadataBytesPerOrg` or `maxMetadataBytesPerAccount`, as the check
    /// before proposing does; a write that shrinks a total passes.
    #[test]
    fn teams_and_metadata_are_bounded_per_organization_and_account() {
        let mut m = machine(Feature::StateCaps.since());
        m.run_authorized(team("org", "t1")).unwrap();
        m.run_authorized(team("org", "t2")).unwrap();
        refused_as_quota(&mut m, team("org", "t3"));
        assert_eq!(m.state.orgs["org"].teams.len(), 2);

        m.run_authorized(org_metadata("org", 60)).unwrap();
        let member = SystemCmd::SetMemberMetadata {
            org: "org".into(),
            account: "alice".into(),
            metadata: json!("y".repeat(48)),
            actor: "alice".into(),
        };
        refused_as_quota(&mut m, member);
        let on_team = SystemCmd::UpdateTeam {
            org: "org".into(),
            team: "t1".into(),
            name: None,
            join: None,
            parent: None,
            metadata: Some(json!("z".repeat(48))),
            actor: "alice".into(),
            all_apps_rows: true,
            close_drops_below: true,
        };
        refused_as_quota(&mut m, on_team);
        assert_eq!(org_metadata_bytes(&m.state.orgs["org"]), 60);

        // The other organization alice owns fits its own bound, not hers: 60 + 100 > 150.
        refused_as_quota(&mut m, org_metadata("org2", 100));
        m.run_authorized(org_metadata("org2", 90)).unwrap();

        // Under a lowered bound, a write that does not grow the total still passes.
        m.state.quotas.max_metadata_bytes_per_org = 10;
        m.run_authorized(org_metadata("org", 60)).unwrap();
        m.run_authorized(org_metadata("org", 20)).unwrap();
        refused_as_quota(&mut m, org_metadata("org", 30));
        m.run_authorized(SystemCmd::SetOrgMetadata {
            org: "org".into(),
            metadata: Value::Null,
            actor: "alice".into(),
        })
        .unwrap();
    }

    /// Below the version that brings them, apply takes what an earlier release took, so a
    /// log written then replays unchanged.
    #[test]
    fn below_their_version_apply_takes_what_it_took() {
        let mut m = machine("1.0.0-rc.38");
        for t in ["t1", "t2", "t3"] {
            m.run_authorized(team("org", t)).unwrap();
        }
        m.run_authorized(org_metadata("org", 200)).unwrap();
        assert_eq!(m.state.orgs["org"].teams.len(), 3);
        // The check before proposing judges by the bounds all the same.
        assert!(m.state.check_state_caps(&team("org", "t4")).is_err());
    }
}
