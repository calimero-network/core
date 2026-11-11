//! The cluster version: the newest entry shapes every node of the cluster
//! applies the same way.
//!
//! Old and new nodes replicate one system log through a rolling upgrade. A
//! node applies an entry it cannot decode as `malformed command` and skips it,
//! so an entry only a newer release understands (a new command, or a field on
//! an old one that changes what it does) would change the state of the newer
//! nodes alone. Each such shape is a [`Feature`] with the release that first
//! applies it; a node refuses to propose a feature above the active version
//! (`409 FEATURE_NOT_ACTIVE`, [`NotActive`]).
//!
//! The active version is the newest version every node supports. The system
//! leader logs it ([`SystemCmd::SetClusterVersion`]) once every registered
//! node advertises it, and it only moves forward: after it, a node below it
//! is not admitted to the system group. Until it is logged (and above it, until
//! the leader logs the next one), a proposing node judges by what the nodes
//! advertise now ([`floor`]). Apply never reads either: an entry applies as
//! it did, so a log replays unchanged.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use serde_json::{json, Value};

use super::{compare_versions, ScopeApp, SystemCmd, SystemState, MAX_RELEASE_VERSION_BYTES};
use crate::types::TeamJoin;

/// An entry shape a node on an earlier release cannot apply as this one does.
/// A new command, or a new field with new meaning, adds one here with the
/// release it first ships in ([`Feature::since`]) and names it in
/// [`SystemCmd::features`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feature {
    /// `SetClusterVersion` itself.
    ClusterVersion,
    /// `MoveApp` (#4907).
    MoveApp,
    /// `ScopeApps` naming one app team's `join` or `keep` (#4903): an earlier
    /// `ScopeApp` refuses unknown fields.
    AppTeamJoin,
    /// `SetOrgQuotas` (#4899).
    OrgQuotas,
    /// `SetPools` (#4898).
    ComputePools,
    /// `UpdateTeam` closing a team with `close_drops_below` (#4911): an
    /// earlier release ignores the field and keeps the rows below.
    TeamCloseDropsBelow,
    /// `MoveApp` with `accounts_in_app_team` (#4912): an earlier release
    /// ignores the field and gives a moved DM's people no app-team row.
    MoveAccountsAppTeam,
    /// `ScopeApps` with `by_release` (#4931): an earlier release judges an
    /// instance's package by its bundle and can put it on another app team.
    ScopeByRelease,
    /// `SetAllApps`, and an invite that gives every app added later
    /// (`Invite::all_apps`) (#4914): an earlier release cannot read the
    /// command and drops the invite's flag.
    AllApps,
}

/// Every feature, oldest first.
pub const FEATURES: [Feature; 9] = [
    Feature::MoveApp,
    Feature::AppTeamJoin,
    Feature::OrgQuotas,
    Feature::ComputePools,
    Feature::ClusterVersion,
    Feature::TeamCloseDropsBelow,
    Feature::MoveAccountsAppTeam,
    Feature::ScopeByRelease,
    Feature::AllApps,
];

impl Feature {
    /// The name a refusal gives it.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::ClusterVersion => "clusterVersion",
            Self::MoveApp => "moveApp",
            Self::AppTeamJoin => "appTeamJoin",
            Self::OrgQuotas => "orgQuotas",
            Self::ComputePools => "computePools",
            Self::TeamCloseDropsBelow => "teamCloseDropsBelow",
            Self::MoveAccountsAppTeam => "moveAccountsAppTeam",
            Self::ScopeByRelease => "scopeByRelease",
            Self::AllApps => "allApps",
        }
    }

    /// The first core release that applies it. The features of a release
    /// still being made name that coming release.
    #[must_use]
    pub const fn since(self) -> &'static str {
        match self {
            Self::MoveApp | Self::AppTeamJoin | Self::OrgQuotas | Self::ComputePools => {
                "1.0.0-rc.15"
            }
            Self::ClusterVersion
            | Self::TeamCloseDropsBelow
            | Self::MoveAccountsAppTeam
            | Self::ScopeByRelease
            | Self::AllApps => "1.0.0-rc.16",
        }
    }
}

/// The newest cluster version this build supports: its release, or the
/// newest feature it knows if that is newer (a build between releases).
/// What `/health` advertises as `clusterVersion`.
#[must_use]
pub fn supported() -> &'static str {
    FEATURES
        .iter()
        .map(|f| f.since())
        .chain([crate::RELEASE_VERSION])
        .max_by(|a, b| compare_versions(a, b))
        .unwrap_or(crate::RELEASE_VERSION)
}

/// A refusal: `feature` needs cluster version `needs`, and the cluster is on
/// `active` (`None`: not every node said which it supports).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotActive {
    pub feature: Feature,
    pub needs: &'static str,
    pub active: Option<String>,
}

impl NotActive {
    /// The refusal's message, naming the feature and the version it needs.
    #[must_use]
    pub fn message(&self) -> String {
        let on = self.active.as_deref().map_or_else(
            || "not every node has said which version it supports".to_owned(),
            |v| format!("the cluster is on {v}"),
        );
        format!(
            "{} needs cluster version {}, and {on}: every node must run {} or newer first",
            self.feature.name(),
            self.needs,
            self.needs
        )
    }

    /// The refusal's details: `{ feature, needs, active }`.
    #[must_use]
    pub fn details(&self) -> Value {
        json!({ "feature": self.feature.name(), "needs": self.needs, "active": self.active })
    }
}

/// Whether every feature in `features` is active at `active`.
///
/// # Errors
/// The first that is not.
pub fn check(active: Option<&str>, features: &[Feature]) -> Result<(), NotActive> {
    for f in features {
        let on = active.is_some_and(|a| compare_versions(a, f.since()) != Ordering::Less);
        if !on {
            return Err(NotActive {
                feature: *f,
                needs: f.since(),
                active: active.map(str::to_owned),
            });
        }
    }
    Ok(())
}

/// The newest version every node supports: the oldest of what each
/// advertises, or `None` when one could not say (or there are none).
#[must_use]
pub fn floor<'a>(advertised: impl IntoIterator<Item = Option<&'a str>>) -> Option<String> {
    let mut low: Option<&str> = None;
    for v in advertised {
        let v = v?;
        if low.is_none_or(|l| compare_versions(v, l) == Ordering::Less) {
            low = Some(v);
        }
    }
    low.map(str::to_owned)
}

/// The version a proposing node judges by: the logged one, or what every
/// node advertises now if that is newer (the leader logs it next).
#[must_use]
pub fn effective(logged: Option<&str>, floor: Option<&str>) -> Option<String> {
    match (logged, floor) {
        (Some(l), Some(f)) if compare_versions(f, l) == Ordering::Greater => Some(f.to_owned()),
        (Some(l), _) => Some(l.to_owned()),
        (None, f) => f.map(str::to_owned),
    }
}

/// What a node advertises in `/health`: `clusterVersion`, or, from a release
/// before it, the core `release` (both are core releases).
#[must_use]
pub fn advertised(health: &Value) -> Option<String> {
    health["clusterVersion"]
        .as_str()
        .or_else(|| health["release"].as_str())
        .filter(|v| v.len() <= MAX_RELEASE_VERSION_BYTES)
        .map(str::to_owned)
}

/// `SetClusterVersion.nodes` on the wire: `[[id, version], ...]`, ids in
/// order (a map with `u64` keys does not read back inside the tagged
/// `SystemCmd`).
pub(super) mod node_versions {
    use std::collections::BTreeMap;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(
        nodes: &BTreeMap<u64, String>,
        s: S,
    ) -> Result<S::Ok, S::Error> {
        nodes.iter().collect::<Vec<_>>().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<BTreeMap<u64, String>, D::Error> {
        let pairs = Vec::<(u64, String)>::deserialize(d)?;
        let len = pairs.len();
        let nodes: BTreeMap<u64, String> = pairs.into_iter().collect();
        if nodes.len() != len {
            return Err(serde::de::Error::custom("a node is named twice"));
        }
        Ok(nodes)
    }
}

/// Refusals of [`SystemCmd::SetClusterVersion`].
pub const NOT_FORWARD: &str = "the cluster version only moves forward";
pub const NOT_EVERY_NODE: &str =
    "a cluster version names what every registered node supports, and only those nodes";
pub const ABOVE_A_NODE: &str = "the cluster version is above what a node supports";
pub const BAD_VERSION: &str = "a cluster version is a semver release";

/// Apply `SetClusterVersion`: `nodes` is what each registered node (and no
/// other) advertised, and `version` is at most the oldest of them and newer
/// than the active version. Judged from the entry and the state alone, so
/// every replica decides the same.
pub(super) fn apply(
    s: &mut SystemState,
    version: String,
    nodes: &BTreeMap<u64, String>,
) -> Result<Value, &'static str> {
    if version.len() > MAX_RELEASE_VERSION_BYTES
        || semver::Version::parse(&version).is_err()
        || nodes.values().any(|v| v.len() > MAX_RELEASE_VERSION_BYTES)
    {
        return Err(BAD_VERSION);
    }
    let registered: Vec<u64> = s
        .replica_nodes()
        .map(|n| n.id)
        .filter(|id| !s.removed_nodes.contains(id))
        .collect();
    if registered.is_empty() || !nodes.keys().copied().eq(registered.iter().copied()) {
        return Err(NOT_EVERY_NODE);
    }
    if nodes
        .values()
        .any(|v| compare_versions(v, &version) == Ordering::Less)
    {
        return Err(ABOVE_A_NODE);
    }
    if s.cluster_version
        .as_deref()
        .is_some_and(|c| compare_versions(&version, c) != Ordering::Greater)
    {
        return Err(NOT_FORWARD);
    }
    s.cluster_version = Some(version.clone());
    Ok(json!({ "clusterVersion": version }))
}

impl SystemCmd {
    /// The features this entry uses: none for a shape every 1.0 release
    /// applies alike. Every command is listed, so a new one decides here.
    #[must_use]
    pub fn features(&self) -> Vec<Feature> {
        match self {
            Self::SetClusterVersion { .. } => vec![Feature::ClusterVersion],
            Self::MoveApp {
                accounts_in_app_team,
                ..
            } => {
                let mut f = vec![Feature::MoveApp];
                if *accounts_in_app_team {
                    f.push(Feature::MoveAccountsAppTeam);
                }
                f
            }
            Self::SetOrgQuotas { .. } => vec![Feature::OrgQuotas],
            Self::SetPools { .. } => vec![Feature::ComputePools],
            Self::ScopeApps {
                apps, by_release, ..
            } => {
                let mut f = scope_app_features(apps.as_deref());
                if *by_release {
                    f.push(Feature::ScopeByRelease);
                }
                f
            }
            // Only a close changes what the flag does.
            Self::UpdateTeam {
                join: Some(TeamJoin::Invite),
                close_drops_below: true,
                ..
            } => vec![Feature::TeamCloseDropsBelow],
            Self::SetAllApps { .. } => vec![Feature::AllApps],
            Self::CreateInvite { invite, .. } if invite.all_apps => vec![Feature::AllApps],
            Self::RegisterNode { .. }
            | Self::RemoveNode { .. }
            | Self::PutBundle { .. }
            | Self::InstallPackage { .. }
            | Self::UninstallPackage { .. }
            | Self::CreateApp { .. }
            | Self::UpgradeApp { .. }
            | Self::TransferApp { .. }
            | Self::DeleteApp { .. }
            | Self::ChargeFileBytes { .. }
            | Self::AcquireRun { .. }
            | Self::ReleaseRun { .. }
            | Self::SetReplicas { .. }
            | Self::SetAccess { .. }
            | Self::CreateAccount { .. }
            | Self::AddPasskey { .. }
            | Self::RemovePasskey { .. }
            | Self::RevokeAllSessions { .. }
            | Self::RefreshReused { .. }
            | Self::SetSignCount { .. }
            | Self::CreateApiKey { .. }
            | Self::RevokeApiKey { .. }
            | Self::SetAccountRecoveryKey { .. }
            | Self::RemoveAccountRecoveryKey { .. }
            | Self::StartAccountRecovery { .. }
            | Self::CancelAccountRecovery { .. }
            | Self::FinishAccountRecovery { .. }
            | Self::CreateRefresh { .. }
            | Self::UseRefresh { .. }
            | Self::RevokeSession { .. }
            | Self::SetQuotas { .. }
            | Self::CreateOrg { .. }
            | Self::Genesis { .. }
            | Self::SetOperatorRelease { .. }
            | Self::SetReleaseApproval { .. }
            | Self::BindPublisher { .. }
            | Self::SetReleaseDeadline { .. }
            | Self::GrantDeployer { .. }
            | Self::RevokeDeployer { .. }
            | Self::SetDeploymentsGranted { .. }
            | Self::SetSignUp { .. }
            | Self::CreateSignUpInvite { .. }
            | Self::RevokeSignUpInvite { .. }
            | Self::SetClusterOwner { .. }
            | Self::CreateDeployment { .. }
            | Self::OfferApp { .. }
            | Self::WithdrawApp { .. }
            | Self::AddPackageOrigin { .. }
            | Self::RemovePackageOrigin { .. }
            | Self::DeleteDeployment { .. }
            | Self::SetOutboundApproval { .. }
            | Self::DeleteOrg { .. }
            | Self::SetOrgMetadata { .. }
            | Self::SetMemberMetadata { .. }
            | Self::SetAppMetadata { .. }
            | Self::SetPasskeyLabel { .. }
            | Self::UpgradeFailed { .. }
            | Self::StartUpgradeRun { .. }
            | Self::FinishUpgradeRun { .. }
            | Self::SetAlias { .. }
            | Self::SetOrgRole { .. }
            | Self::RemoveOrgMember { .. }
            | Self::CreateTeam { .. }
            | Self::SetTeamMember { .. }
            | Self::UpdateTeam { .. }
            | Self::DeleteTeam { .. }
            | Self::SetCapabilities { .. }
            | Self::SetDefaultCapabilities { .. }
            | Self::RemoveTeamMember { .. }
            | Self::CreateInvite { .. }
            | Self::RedeemInvite { .. }
            | Self::RevokeInvite { .. }
            | Self::SetRecoveryKey { .. }
            | Self::StartRecovery { .. }
            | Self::CancelRecovery { .. }
            | Self::FinishRecovery { .. }
            | Self::RequireSessionProofs => Vec::new(),
        }
    }
}

/// Whether a node advertising `advertised` may join the system group at the
/// logged cluster version: one below it (or that does not say) would skip
/// entries every other node applies. Before any version is logged, every
/// node may.
#[must_use]
pub fn may_join(logged: Option<&str>, advertised: Option<&str>) -> bool {
    logged.is_none_or(|l| advertised.is_some_and(|a| compare_versions(a, l) != Ordering::Less))
}

/// The features a `ScopeApps` with `apps` uses: one app team's `join` or
/// `keep` ([`Feature::AppTeamJoin`]).
#[must_use]
pub fn scope_app_features(apps: Option<&[ScopeApp]>) -> Vec<Feature> {
    let joins = apps
        .into_iter()
        .flatten()
        .any(|a| a.join.is_some() || a.keep.is_some());
    if joins {
        vec![Feature::AppTeamJoin]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::{json, Value};

    use super::{
        advertised, check, effective, floor, may_join, supported, Feature, ABOVE_A_NODE, FEATURES,
        NOT_EVERY_NODE, NOT_FORWARD,
    };
    use crate::system::{compare_versions, OrgQuotas, ScopeApp, SystemCmd, SystemMachine};
    use crate::types::TeamJoin;

    const RC14: &str = "1.0.0-rc.14";
    const RC15: &str = "1.0.0-rc.15";
    const RC16: &str = "1.0.0-rc.16";

    /// An entry that uses `feature`, as a node proposes it.
    fn uses(feature: Feature) -> SystemCmd {
        match feature {
            Feature::ClusterVersion => SystemCmd::SetClusterVersion {
                version: RC16.into(),
                nodes: (1..=3).map(|id| (id, RC16.to_owned())).collect(),
            },
            Feature::MoveApp => SystemCmd::MoveApp {
                app_id: "app".into(),
                to_org: "org-b".into(),
                actor: "alice".into(),
                consent: Value::Null,
                bring_members: false,
                nonce: "n".into(),
                accounts_in_app_team: false,
                all_apps_rows: false,
            },
            Feature::MoveAccountsAppTeam => SystemCmd::MoveApp {
                app_id: "app".into(),
                to_org: "org-b".into(),
                actor: "alice".into(),
                consent: Value::Null,
                bring_members: true,
                nonce: "n".into(),
                accounts_in_app_team: true,
                all_apps_rows: false,
            },
            Feature::AppTeamJoin => SystemCmd::ScopeApps {
                org: "org".into(),
                apps: Some(vec![ScopeApp {
                    package: "chat".into(),
                    name: None,
                    instances: None,
                    join: Some(TeamJoin::Invite),
                    keep: None,
                }]),
                shared: None,
                nonce: "n".into(),
                actor: "alice".into(),
                consent: Value::Null,
                owner_in_team: true,
                accounts_in_app_team: true,
                member_child_teams: true,
                by_release: false,
                all_apps_rows: false,
            },
            Feature::ScopeByRelease => SystemCmd::ScopeApps {
                org: "org".into(),
                apps: None,
                shared: Some(true),
                nonce: "n".into(),
                actor: "alice".into(),
                consent: Value::Null,
                owner_in_team: true,
                accounts_in_app_team: true,
                member_child_teams: true,
                by_release: true,
                all_apps_rows: false,
            },
            Feature::AllApps => SystemCmd::SetAllApps {
                org: "org".into(),
                account: "bob".into(),
                all_apps: true,
                actor: "alice".into(),
                consent: Value::Null,
            },
            Feature::OrgQuotas => SystemCmd::SetOrgQuotas {
                payer: "org".into(),
                quotas: OrgQuotas {
                    max_concurrent_runs: Some(3),
                    ..OrgQuotas::default()
                },
                proof: Value::Null,
            },
            Feature::ComputePools => SystemCmd::SetPools {
                pools: BTreeMap::new(),
                proof: Value::Null,
            },
            Feature::TeamCloseDropsBelow => SystemCmd::UpdateTeam {
                org: "org".into(),
                team: "team".into(),
                name: None,
                join: Some(TeamJoin::Invite),
                parent: None,
                metadata: None,
                actor: "alice".into(),
                close_drops_below: true,
                all_apps_rows: false,
            },
        }
    }

    /// A node of the cluster: the release it runs and its system state.
    struct Replica {
        release: &'static str,
        machine: SystemMachine,
    }

    impl Replica {
        /// Apply `cmd` as a node on this release does: an entry it cannot
        /// decode (it uses a feature newer than the release) is skipped as
        /// `malformed command`, as `SystemMachine::apply` does.
        fn apply(&mut self, cmd: &SystemCmd) -> Result<Value, String> {
            if check(Some(self.release), &cmd.features()).is_err() {
                return Err("malformed command".into());
            }
            self.machine
                .run_authorized(cmd.clone())
                .map_err(str::to_owned)
        }

        fn state(&self) -> Value {
            serde_json::to_value(&self.machine.state).unwrap()
        }
    }

    /// Nodes on `releases` replicating one system log, registered as nodes 1, 2, ...
    fn cluster(releases: &[&'static str]) -> Vec<Replica> {
        let mut nodes: Vec<Replica> = releases
            .iter()
            .map(|&release| Replica {
                release,
                machine: SystemMachine::default(),
            })
            .collect();
        for id in 1..=releases.len() as u64 {
            let register = SystemCmd::RegisterNode {
                node: serde_json::from_value(
                    json!({ "id": id, "addr": format!("http://n{id}"), "role": "replica" }),
                )
                .unwrap(),
                proof: Value::Null,
            };
            for n in &mut nodes {
                let _ok = n.apply(&register).unwrap();
            }
        }
        nodes
    }

    /// Propose `cmd` through the gate a node runs before it proposes
    /// (`Node::require_features`): the logged version, or what every node
    /// advertises now. Admitted, every node applies it.
    fn propose(nodes: &mut [Replica], cmd: &SystemCmd) -> Result<(), super::NotActive> {
        let logged = nodes[0].machine.state.cluster_version.clone();
        let low = floor(nodes.iter().map(|n| Some(n.release)));
        check(
            effective(logged.as_deref(), low.as_deref()).as_deref(),
            &cmd.features(),
        )?;
        for n in nodes.iter_mut() {
            let _answer = n.apply(cmd);
        }
        Ok(())
    }

    fn same_state(nodes: &[Replica]) -> bool {
        nodes.windows(2).all(|w| w[0].state() == w[1].state())
    }

    /// Reproduction: in a cluster rolling from rc.14, a node on rc.15 logs
    /// an override rc.14 cannot decode. The rc.15 nodes apply it, the rc.14
    /// node skips it, and their states differ from then on.
    #[test]
    fn an_entry_an_older_node_cannot_decode_splits_the_state() {
        let mut nodes = cluster(&[RC15, RC15, RC14]);
        let cmd = uses(Feature::OrgQuotas);
        let answers: Vec<Result<Value, String>> = nodes.iter_mut().map(|n| n.apply(&cmd)).collect();
        assert!(answers[0].is_ok() && answers[1].is_ok(), "{answers:?}");
        assert_eq!(answers[2], Err("malformed command".into()));
        assert!(
            !same_state(&nodes),
            "the older node kept the state before it"
        );
    }

    /// What an rc.14 node decodes `ScopeApp` as: it refuses unknown fields,
    /// so a `ScopeApps` naming one team's `join` is malformed there.
    #[test]
    fn rc14_refuses_a_scope_app_naming_join() {
        #[derive(Debug, Deserialize)]
        #[serde(rename_all = "camelCase", deny_unknown_fields)]
        #[allow(dead_code, reason = "a copy of rc.14's type, decoded only")]
        struct Rc14ScopeApp {
            package: String,
            #[serde(default)]
            name: Option<String>,
            #[serde(default)]
            instances: Option<Vec<String>>,
        }
        let SystemCmd::ScopeApps {
            apps: Some(apps), ..
        } = uses(Feature::AppTeamJoin)
        else {
            unreachable!()
        };
        let entry = serde_json::to_value(&apps[0]).unwrap();
        assert!(serde_json::from_value::<Rc14ScopeApp>(entry).is_err());
        let plain = ScopeApp {
            join: None,
            ..apps[0].clone()
        };
        let entry = serde_json::to_value(&plain).unwrap();
        assert!(serde_json::from_value::<Rc14ScopeApp>(entry).is_ok());
        assert!(super::scope_app_features(Some(&[plain])).is_empty());
    }

    /// While an older node replicates the system log, no feature newer than
    /// it is proposed, so every node keeps the same state; once it leaves,
    /// the leader raises the cluster version and the features work.
    #[test]
    fn a_feature_is_never_proposed_while_an_older_node_holds_the_log() {
        let mut nodes = cluster(&[RC16, RC16, RC14]);
        for feature in FEATURES {
            let cmd = uses(feature);
            let needs = cmd.features();
            assert!(needs.contains(&feature), "{feature:?}: {needs:?}");
            let refused = propose(&mut nodes, &cmd).unwrap_err();
            // The first feature the entry needs that the cluster lacks: a move
            // carrying `accounts_in_app_team` is refused as `moveApp` here.
            assert_eq!(refused.feature, needs[0]);
            assert_eq!(refused.active.as_deref(), Some(RC14));
            assert!(
                refused.message().contains(refused.feature.since()),
                "{}",
                refused.message()
            );
            assert!(same_state(&nodes));
        }
        // Commands every release applies alike still go through.
        let mut plain = uses(Feature::AppTeamJoin);
        if let SystemCmd::ScopeApps { apps, .. } = &mut plain {
            *apps = None;
        }
        assert!(propose(&mut nodes, &plain).is_ok());
        assert!(same_state(&nodes));

        // The rc.14 node is replaced by one on rc.16 (mdma adds the new
        // node, then removes the old one).
        nodes[2].release = RC16;
        let raise = uses(Feature::ClusterVersion);
        propose(&mut nodes, &raise).unwrap();
        assert!(nodes
            .iter()
            .all(|n| n.machine.state.cluster_version.as_deref() == Some(RC16)));
        for feature in FEATURES {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        assert!(same_state(&nodes));
        assert!(nodes[0].machine.state.org_quotas.contains_key("org"));
    }

    /// Between rc.15 and rc.16 the rc.15 features stay on: the floor is
    /// rc.15 though nothing is logged, as on a cluster all on rc.15.
    #[test]
    fn a_cluster_from_rc15_keeps_its_features_while_it_rolls() {
        let mut nodes = cluster(&[RC16, RC15, RC15]);
        for feature in [
            Feature::MoveApp,
            Feature::AppTeamJoin,
            Feature::OrgQuotas,
            Feature::ComputePools,
        ] {
            assert!(propose(&mut nodes, &uses(feature)).is_ok(), "{feature:?}");
        }
        assert!(propose(&mut nodes, &uses(Feature::ClusterVersion)).is_err());
        // rc.16's fields on an rc.15 command are held back by their own name.
        for feature in [
            Feature::TeamCloseDropsBelow,
            Feature::MoveAccountsAppTeam,
            Feature::ScopeByRelease,
            Feature::AllApps,
        ] {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(refused.feature, feature);
            assert_eq!(refused.active.as_deref(), Some(RC15));
        }
        assert!(same_state(&nodes));
    }

    #[test]
    fn the_logged_version_moves_forward_only_and_names_every_node() {
        let mut nodes = cluster(&[RC16, RC16, RC16]);
        let m = &mut nodes[0].machine;
        let set = |version: &str, nodes: &[(u64, &str)]| SystemCmd::SetClusterVersion {
            version: version.into(),
            nodes: nodes.iter().map(|(id, v)| (*id, (*v).to_owned())).collect(),
        };
        // Every registered node, and only those.
        let two = [(1, RC16), (2, RC16)];
        assert_eq!(m.run_authorized(set(RC16, &two)), Err(NOT_EVERY_NODE));
        let four = [(1, RC16), (2, RC16), (3, RC16), (4, RC16)];
        assert_eq!(m.run_authorized(set(RC16, &four)), Err(NOT_EVERY_NODE));
        // Never above what a node supports.
        let behind = [(1, RC16), (2, RC16), (3, RC15)];
        assert_eq!(m.run_authorized(set(RC16, &behind)), Err(ABOVE_A_NODE));
        assert!(m.run_authorized(set("not a version", &behind)).is_err());
        let _ok = m.run_authorized(set(RC15, &behind)).unwrap();
        assert_eq!(m.state.cluster_version.as_deref(), Some(RC15));
        let all = [(1, RC16), (2, RC16), (3, RC16)];
        let _ok = m.run_authorized(set(RC16, &all)).unwrap();
        // Never back, never the same again.
        assert_eq!(m.run_authorized(set(RC16, &all)), Err(NOT_FORWARD));
        assert_eq!(m.run_authorized(set(RC15, &all)), Err(NOT_FORWARD));
        assert_eq!(m.state.cluster_version.as_deref(), Some(RC16));
        // A removed node is not named.
        let _ok = m
            .run_authorized(SystemCmd::RemoveNode {
                id: 3,
                proof: Value::Null,
            })
            .unwrap();
        let rc17 = [(1, "1.0.0-rc.17"), (2, "1.0.0-rc.17")];
        let _ok = m.run_authorized(set("1.0.0-rc.17", &rc17)).unwrap();
    }

    /// Reproduction: `SetClusterVersion` was logged as `{"1": ...}` and every
    /// node refused it ("invalid type: string \"1\", expected u64"), since
    /// the tagged `SystemCmd` reads map keys back as strings. It must read
    /// back as the command it was.
    #[test]
    fn set_cluster_version_reads_back_from_the_log() {
        let cmd = uses(Feature::ClusterVersion);
        let bytes = serde_json::to_vec(&cmd).unwrap();
        let back: SystemCmd = serde_json::from_slice(&bytes).unwrap();
        let (
            SystemCmd::SetClusterVersion { version, nodes },
            SystemCmd::SetClusterVersion {
                version: v2,
                nodes: n2,
            },
        ) = (&cmd, &back)
        else {
            panic!("{back:?}");
        };
        assert_eq!((version, nodes), (v2, n2));
        let twice =
            json!({ "op": "setClusterVersion", "version": RC16, "nodes": [[1, RC16], [1, RC16]] });
        assert!(serde_json::from_value::<SystemCmd>(twice).is_err());
    }

    #[test]
    fn state_without_a_cluster_version_reads_and_writes_as_before() {
        let m = SystemMachine::default();
        let v = serde_json::to_value(&m.state).unwrap();
        assert!(v.get("clusterVersion").is_none());
        let back: crate::system::SystemState = serde_json::from_value(v).unwrap();
        assert!(back.cluster_version.is_none());
    }

    #[test]
    fn the_floor_waits_for_every_node() {
        assert_eq!(
            floor([Some(RC16), Some(RC15), Some(RC16)]),
            Some(RC15.into())
        );
        assert_eq!(
            floor([Some(RC16), None]),
            None,
            "a node that did not answer"
        );
        assert_eq!(floor(std::iter::empty()), None);
        // rc.9 is older than rc.15 (numeric pre-release order).
        assert_eq!(
            floor([Some(RC15), Some("1.0.0-rc.9")]),
            Some("1.0.0-rc.9".into())
        );
        assert_eq!(effective(Some(RC16), Some(RC15)), Some(RC16.into()));
        assert_eq!(effective(Some(RC15), Some(RC16)), Some(RC16.into()));
        assert_eq!(effective(None, Some(RC15)), Some(RC15.into()));
        assert_eq!(effective(None, None), None);
        assert!(check(None, &[Feature::MoveApp]).is_err());
        assert!(check(None, &[]).is_ok());
    }

    #[test]
    fn a_node_below_the_logged_version_does_not_join() {
        assert!(may_join(None, None));
        assert!(may_join(Some(RC15), Some(RC16)));
        assert!(may_join(Some(RC16), Some(RC16)));
        assert!(!may_join(Some(RC16), Some(RC15)));
        assert!(!may_join(Some(RC16), None));
    }

    /// A node on this release advertises `clusterVersion`; one before it,
    /// only its core `release`, which the gate reads in its place.
    #[test]
    fn what_a_node_advertises() {
        assert_eq!(
            advertised(&json!({ "release": RC15, "clusterVersion": RC16 })),
            Some(RC16.into())
        );
        assert_eq!(advertised(&json!({ "release": RC14 })), Some(RC14.into()));
        assert_eq!(advertised(&json!({ "waiting": "key" })), None);
        assert_eq!(advertised(&json!({ "release": "x".repeat(65) })), None);
    }

    /// This build supports every feature it knows and its own release.
    /// An invite is an older shape unless it gives every app added later,
    /// which an earlier release would drop.
    #[test]
    fn only_an_invite_giving_every_later_app_needs_all_apps() {
        let invite = |all_apps: bool| SystemCmd::CreateInvite {
            invite_key: "k".into(),
            invite: serde_json::from_value(json!({
                "org": "org", "role": "member", "expiresAt": 1, "usesLeft": 1,
                "allApps": all_apps,
            }))
            .unwrap(),
            actor: "alice".into(),
            consent: Value::Null,
            teams_seen: true,
        };
        assert!(invite(false).features().is_empty());
        assert_eq!(invite(true).features(), [Feature::AllApps]);
    }

    #[test]
    fn this_build_supports_its_features() {
        for f in FEATURES {
            assert!(check(Some(supported()), &[f]).is_ok(), "{f:?}");
            assert!(semver::Version::parse(f.since()).is_ok(), "{f:?}");
        }
        assert_ne!(
            compare_versions(supported(), crate::RELEASE_VERSION),
            std::cmp::Ordering::Less
        );
        let names: std::collections::BTreeSet<&str> = FEATURES.iter().map(|f| f.name()).collect();
        assert_eq!(names.len(), FEATURES.len());
    }
}
