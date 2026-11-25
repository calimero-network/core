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
//! advertise now ([`floor`]). Apply reads only the logged version at the entry's own
//! position ([`SystemState::cluster_brings`]), so a log replays unchanged.

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
    /// `MoveApp`.
    MoveApp,
    /// `ScopeApps` naming one app team's `join` or `keep`: an earlier
    /// `ScopeApp` refuses unknown fields.
    AppTeamJoin,
    /// `SetOrgQuotas`.
    OrgQuotas,
    /// `SetPools`.
    ComputePools,
    /// `UpdateTeam` closing a team with `close_drops_below`: an
    /// earlier release ignores the field and keeps the rows below.
    TeamCloseDropsBelow,
    /// `MoveApp` with `accounts_in_app_team`: an earlier release
    /// ignores the field and gives a moved DM's people no app-team row.
    MoveAccountsAppTeam,
    /// `ScopeApps` with `by_release`: an earlier release judges an
    /// instance's package by its bundle and can put it on another app team.
    ScopeByRelease,
    /// `SetAllApps`, and an invite that gives every app added later
    /// (`Invite::all_apps`): an earlier release cannot read the
    /// command and drops the invite's flag.
    AllApps,
    /// `RestoreApp` and `FinishRestore`: an earlier release cannot
    /// read them, so its replicas keep the app where it was.
    RestoreApp,
    /// `RenameOrg`: an earlier release cannot read it, so its
    /// replicas keep the old name.
    RenameOrg,
    /// `SetTeamMembers` and `RemoveTeamMembers`: an earlier release
    /// cannot read them.
    TeamMembersBatch,
    /// `ScopeApps` naming one app team's `childTeams`: an earlier
    /// `ScopeApp` refuses unknown fields.
    AppTeamChildTeams,
    /// `SetPriceTable`: an earlier release cannot read it.
    PriceTable,
    /// `InstallPackage` or `CreateApp` of a release with only a compute component: an earlier
    /// release ignores `components` and treats it as an app.
    ComputeOnly,
    /// `InstallPackage` of a release that registers pipelines: an earlier release drops them.
    Pipelines,
    /// `AcquireRun` naming a pool: an earlier release ignores `pool` and places by memory.
    NamedPool,
    /// `AttributeByRelease`: an earlier release cannot read it, so
    /// its replicas would keep judging apps by the release each bundle was
    /// first recorded from.
    AttributeByRelease,
    /// `RequireCurrentRules`: an earlier release cannot read it, so
    /// its replicas would keep applying later entries' flags as logged.
    CurrentRules,
    /// `SetBackupKey` and `ImportHolder`: an earlier release cannot read
    /// them, so its replicas keep no key and import nothing.
    BackupKeys,
    /// `CreateOrg` starting with an app team (`app_team`): an earlier
    /// release ignores the field and makes the organization without it.
    OrgAppTeam,
    /// `MoveApp` with `org_quotas`: an earlier release ignores the field and
    /// judges the target by the cluster's limits, not its own override.
    MoveOrgQuotas,
    /// `StartUpgradeRun` with `by_release`: an earlier release ignores the
    /// field and judges a publisher's run by each bundle's first package.
    PublishedRunByRelease,
    /// `CreateOrg` with `capabilities`: an earlier release ignores the field
    /// and leaves the organization's member default as a new one's.
    OrgCapabilities,
    /// `CreateInvite` with `capabilities`: an earlier release drops the field,
    /// so a member redeeming it would get the organization's default.
    InviteCapabilities,
    /// `InstallPackage` of a release that declares `secrets`: an earlier release drops
    /// them and fills any secret its operator bound to the package.
    DeclaredSecrets,
    /// `StartUpgradeRun` of a unit, whose members each have their own target
    /// (`UpgradeRun::targets`): an earlier release drops the targets and would refuse each
    /// member's `UpgradeApp` as not in the run.
    UnitUpgrade,
    /// `CreateApp` naming the apps its release created with it (`AppRecord::links`): an
    /// earlier release drops them, so its replicas would list the instance without its unit.
    AppLinks,
    /// `SetDelegation`, a person letting an app call another as them
    /// (`SystemState::delegations`): an earlier release cannot decode it, so its replicas
    /// would hold no grant and deliver the same call as the app on one leader and as the
    /// person on another.
    Delegation,
    /// `LinkApp`, an app attached to an instance's `attach` reference
    /// (`AppRecord::attached`): an earlier release cannot decode it, so its replicas would
    /// list the instance without the app and its runs would read no link.
    AttachApp,
    /// `ExpireProofs`: an earlier release cannot read it, so its replicas
    /// would keep every sign-in's proof and accept a sign-in this release
    /// refuses as expired.
    ExpireProofs,
    /// `SetDeploymentSecret` and `ClearDeploymentSecret`, a deployment's managers
    /// setting a secret's value for every instance created from it
    /// (`SystemState::deployment_secrets`): an earlier release cannot decode them, so
    /// an instance whose leader is on it would send the operator's value, or none, where
    /// another leader sends the deployment's.
    DeploymentSecrets,
    /// A unit is one: `CreateApp` of a unit's member carrying its instance's deployment
    /// (`unit_offer`, the offer that covers it), and `DeleteApp` taking an instance's unit
    /// with it (`members`): an earlier release ignores both fields, so its replicas would
    /// refuse the member as not offered and keep the members of a deleted instance.
    WholeUnits,
    /// `SetDeploymentSecret` with `used` (a name a release an offered release references
    /// declares), and `InstallPackage` recording a release's `uses`: an earlier release
    /// refuses the one as undeclared and drops the other, so its replicas would hold no value
    /// where the others hold one.
    DeploymentUsedSecrets,
    /// `UpgradeApp` with `in_deployment`: an instance from a deployment moves only to the
    /// release its deployment offers. An earlier release ignores the field and would move
    /// it where a later one refuses.
    UpgradeInDeployment,
    /// Ordinary organizations: a sign-up makes no organization, a personal one is no
    /// longer made (`CreateOrg { personal }` is refused) nor treated apart (it counts
    /// against `maxOrgsPerAccount`, and an app there is transferred only to a member), and
    /// `InstallPackage` records the release's instance mode (`PackageVersion::instance_org`,
    /// the manifest's `instanceOrg`). Apply reads the logged version for the rules
    /// (`SystemState::ordinary_orgs`), so an entry before it replays as it applied; an
    /// earlier release drops the recorded mode, so it is recorded only from this version.
    OrdinaryOrgs,
    /// `UpgradeApp`, `StartUpgradeRun` and `CreateApiKey` with `owner_approves`: only an
    /// owner or admin of the app's organization approves its upgrades
    /// (`SystemState::approves_upgrades`). An earlier release ignores the field and would
    /// apply a member's, a team lead's or a publisher's upgrade where a later one refuses it.
    OwnerApprovedUpgrades,
    /// Guests: an instance's listed accounts reach it without joining its organization
    /// where its release (`PackageVersion::guests`) and its organization (`allowGuests`)
    /// allow; `SetOrgSettings { allow_guests }`, and instance invites (`CreateAppInvite`, `RevokeAppInvite`,
    /// `RedeemAppInvite`); `InstallPackage` recording the release's `guests`. An earlier
    /// release cannot decode the commands, drops the field, and would refuse a guest the
    /// others admit. Apply reads the logged version for the reach rule
    /// (`SystemState::guests_on`), so state before it reads as it did.
    Guests,
    /// An instance's access as a list of grants (`Access`: the organization, teams, named
    /// members and guests with an app-defined scope, anyone): every `SetAccess`, a `CreateApp`
    /// with any grant, and the instance invites, whose redemption adds a grant. An earlier
    /// release reads access as one mode: it cannot decode the list, and reads an instance
    /// without one as its owner's alone.
    AccessGrants,
    /// `DeleteTeam` keeping the instances only the team admitted (`keep_apps`): they lose its
    /// grants and keep the rest. An earlier release ignores the field and deletes them.
    KeepTeamApps,
    /// Public links: `CreatePublicLink` and `RevokePublicLink` (an instance's link that
    /// lets anonymous visitors call its `#[app::public]` methods), and `SetOrgSettings`
    /// naming `allowPublicLinks`. An earlier release cannot decode the commands or drops
    /// the setting; a visitor's call names `Caller::Visitor` in the app's log, which an
    /// earlier replica cannot decode, so visitors are admitted only at this version too.
    PublicLinks,
    /// `UpgradeApp` with `class`: what the update classifier said of the move, judged at
    /// apply against the log (`SystemState::check_class`) and recorded on the app's
    /// `lastUpgrade`. An earlier release ignores the field and would apply a move logged as
    /// green that the log shows red, where a later one refuses it.
    UpgradeClasses,
    /// Update policies and second approvers: `SetUpgradePolicy` and `ApproveUpgrade`
    /// (`types::UpgradePolicy`, `types::PendingApproval`). An earlier release cannot decode
    /// them, so it would keep no policy and apply a red update with one approval where a
    /// later one waits for the second.
    UpgradePolicies,
    /// `UpgradeApp { auto }`: a green update of an app on a `noDestinations` policy
    /// applies on its own once its hold passed, proposed by the system leader and judged
    /// at apply from the log (`SystemState::auto_update_gate`). An earlier release would
    /// take it as an upgrade by the actor it names, which is no account, and refuse it.
    AutoUpdates,
    /// `UpgradeApp { restore_point }` and `UndoUpgrade`: the restore point an upgrade
    /// takes, kept on the app, and the undo back to it. An earlier release would keep no
    /// restore point and could not decode the undo.
    RestorePoints,
    /// `ApproveUpgrade` and `UpgradeApp` whose class carries an `unclassified` reason
    /// (`calimero_bundle::update::Reason::Unclassified`): a move the log cannot classify,
    /// a part's release no longer installed, approved first and applied as red. rc.34
    /// cannot decode the reason, and a node stops a group on an entry it cannot decode.
    UnclassifiedUpdates,
    /// `InstallPackage` whose release carries `visibility` or `orgs`: who may use it. An
    /// earlier release drops both and would let anyone sharing an organization with an
    /// installer, or reaching an app of it, use a private release.
    PrivateReleases,
    /// `CreateApp { offers_only }`: where deployments are granted, an instance from no
    /// deployment's offer is a deployer's (`SystemState::creates_outside_offers`). An earlier
    /// release ignores the field and would create one a later one refuses. Apply also reads the
    /// logged version for the rule, so an entry before it replays as it applied; from that
    /// version an `UpgradeApp` moving such an instance into another package is judged alike
    /// (`SystemState::upgrades_outside_offers`).
    OffersOnly,
    /// `UndoUpgrade { release_installed }`: an undo goes back only to an installed release;
    /// an earlier release would apply one to a release uninstalled under a shared bundle.
    UndoToInstalledRelease,
    /// `UpgradeApp { auto, members_stay }`: an update on its own moves the instance alone,
    /// so it is refused where its release would move the unit's members
    /// (`SystemState::moves_members`). An earlier release would apply it and leave them behind.
    AutoUpdateMembersStay,
    /// `CreateApiKey { then }`: a key whose consent also approved confirm calls. An earlier
    /// release rebuilds the statement without them and refuses the key a later one makes.
    KeyFollowUps,
    /// Below admin, a team's row, a row's capabilities and a team's default are set only on a
    /// team the actor sees, and grant only capabilities the actor holds organization-wide
    /// (`set_team_row`, `SetCapabilities`, `SetDefaultCapabilities`), as an invite's are. No
    /// entry changes shape: apply reads the logged version, so a log replays unchanged, and an
    /// earlier release, which would apply what this one refuses, holds it below the version.
    HeldTeamGrants,
    /// An invite hands out only what its creator could still hand out when it is redeemed: an
    /// organization invite's creator no longer a member, below the invite's role, or no longer
    /// seeing a team it names, gives nothing (`SystemState::redeem_org_invite`), and an
    /// instance invite's creator who no longer manages the instance's grants names nobody
    /// (`system::apps::redeem_app_invite`). No entry changes shape.
    InviteCreatorStanding,
    /// An organization's backup key is set only by those who approve its upgrades, its owner
    /// and admins (`SystemState::may_set_backup_key`), as `OwnerApprovedUpgrades` made
    /// upgrades theirs. No entry changes shape.
    BackupKeyApprovers,
}

/// Every feature, oldest first.
pub const FEATURES: [Feature; 54] = [
    Feature::MoveApp,
    Feature::AppTeamJoin,
    Feature::OrgQuotas,
    Feature::ComputePools,
    Feature::ClusterVersion,
    Feature::TeamCloseDropsBelow,
    Feature::MoveAccountsAppTeam,
    Feature::ScopeByRelease,
    Feature::AllApps,
    Feature::RestoreApp,
    Feature::RenameOrg,
    Feature::TeamMembersBatch,
    Feature::AppTeamChildTeams,
    Feature::PriceTable,
    Feature::ComputeOnly,
    Feature::Pipelines,
    Feature::NamedPool,
    Feature::AttributeByRelease,
    Feature::CurrentRules,
    Feature::BackupKeys,
    Feature::OrgAppTeam,
    Feature::MoveOrgQuotas,
    Feature::PublishedRunByRelease,
    Feature::OrgCapabilities,
    Feature::InviteCapabilities,
    Feature::DeclaredSecrets,
    Feature::AppLinks,
    Feature::UnitUpgrade,
    Feature::Delegation,
    Feature::AttachApp,
    Feature::ExpireProofs,
    Feature::DeploymentSecrets,
    Feature::WholeUnits,
    Feature::DeploymentUsedSecrets,
    Feature::UpgradeInDeployment,
    Feature::OrdinaryOrgs,
    Feature::OwnerApprovedUpgrades,
    Feature::Guests,
    Feature::AccessGrants,
    Feature::KeepTeamApps,
    Feature::PublicLinks,
    Feature::UpgradeClasses,
    Feature::UpgradePolicies,
    Feature::AutoUpdates,
    Feature::RestorePoints,
    Feature::UnclassifiedUpdates,
    Feature::PrivateReleases,
    Feature::OffersOnly,
    Feature::UndoToInstalledRelease,
    Feature::AutoUpdateMembersStay,
    Feature::KeyFollowUps,
    Feature::HeldTeamGrants,
    Feature::InviteCreatorStanding,
    Feature::BackupKeyApprovers,
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
            Self::RestoreApp => "restoreApp",
            Self::RenameOrg => "renameOrg",
            Self::TeamMembersBatch => "teamMembersBatch",
            Self::AppTeamChildTeams => "appTeamChildTeams",
            Self::PriceTable => "priceTable",
            Self::ComputeOnly => "computeOnly",
            Self::Pipelines => "pipelines",
            Self::NamedPool => "namedPool",
            Self::AttributeByRelease => "attributeByRelease",
            Self::CurrentRules => "currentRules",
            Self::BackupKeys => "backupKeys",
            Self::OrgAppTeam => "orgAppTeam",
            Self::MoveOrgQuotas => "moveOrgQuotas",
            Self::PublishedRunByRelease => "publishedRunByRelease",
            Self::OrgCapabilities => "orgCapabilities",
            Self::InviteCapabilities => "inviteCapabilities",
            Self::DeclaredSecrets => "declaredSecrets",
            Self::AppLinks => "appLinks",
            Self::UnitUpgrade => "unitUpgrade",
            Self::Delegation => "delegation",
            Self::AttachApp => "attachApp",
            Self::ExpireProofs => "expireProofs",
            Self::DeploymentSecrets => "deploymentSecrets",
            Self::WholeUnits => "wholeUnits",
            Self::DeploymentUsedSecrets => "deploymentUsedSecrets",
            Self::UpgradeInDeployment => "upgradeInDeployment",
            Self::OrdinaryOrgs => "ordinaryOrgs",
            Self::OwnerApprovedUpgrades => "ownerApprovedUpgrades",
            Self::Guests => "guests",
            Self::AccessGrants => "accessGrants",
            Self::KeepTeamApps => "keepTeamApps",
            Self::PublicLinks => "publicLinks",
            Self::UpgradeClasses => "upgradeClasses",
            Self::UpgradePolicies => "upgradePolicies",
            Self::AutoUpdates => "autoUpdates",
            Self::RestorePoints => "restorePoints",
            Self::UnclassifiedUpdates => "unclassifiedUpdates",
            Self::PrivateReleases => "privateReleases",
            Self::OffersOnly => "offersOnly",
            Self::UndoToInstalledRelease => "undoToInstalledRelease",
            Self::AutoUpdateMembersStay => "autoUpdateMembersStay",
            Self::KeyFollowUps => "keyFollowUps",
            Self::HeldTeamGrants => "heldTeamGrants",
            Self::InviteCreatorStanding => "inviteCreatorStanding",
            Self::BackupKeyApprovers => "backupKeyApprovers",
        }
    }

    /// Whether an entry carries it ([`SystemCmd::features`] names it). `HeldTeamGrants`,
    /// `InviteCreatorStanding` and `BackupKeyApprovers` are rules apply reads from the logged
    /// version: no entry names them.
    #[must_use]
    pub const fn names_an_entry(self) -> bool {
        !matches!(
            self,
            Self::HeldTeamGrants | Self::InviteCreatorStanding | Self::BackupKeyApprovers
        )
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
            | Self::AllApps
            | Self::RestoreApp
            | Self::RenameOrg
            | Self::TeamMembersBatch
            | Self::AppTeamChildTeams => "1.0.0-rc.16",
            Self::PriceTable
            | Self::ComputeOnly
            | Self::Pipelines
            | Self::NamedPool
            | Self::AttributeByRelease
            | Self::CurrentRules => "1.0.0-rc.19",
            Self::BackupKeys => "1.0.0-rc.20",
            Self::OrgAppTeam => "1.0.0-rc.22",
            Self::MoveOrgQuotas | Self::PublishedRunByRelease => "1.0.0-rc.24",
            Self::OrgCapabilities | Self::InviteCapabilities => "1.0.0-rc.29",
            Self::DeclaredSecrets
            | Self::AppLinks
            | Self::UnitUpgrade
            | Self::Delegation
            | Self::AttachApp
            | Self::ExpireProofs => "1.0.0-rc.31",
            Self::DeploymentSecrets
            | Self::WholeUnits
            | Self::DeploymentUsedSecrets
            | Self::UpgradeInDeployment
            | Self::OrdinaryOrgs => "1.0.0-rc.32",
            Self::OwnerApprovedUpgrades
            | Self::Guests
            | Self::AccessGrants
            | Self::KeepTeamApps
            | Self::PublicLinks => "1.0.0-rc.33",
            Self::UpgradeClasses
            | Self::UpgradePolicies
            | Self::AutoUpdates
            | Self::RestorePoints => "1.0.0-rc.34",
            Self::UnclassifiedUpdates => "1.0.0-rc.35",
            Self::OffersOnly | Self::UndoToInstalledRelease | Self::AutoUpdateMembersStay => {
                "1.0.0-rc.36"
            }
            // Key follow-ups merged after 1.0.0-rc.37 was cut, so rc.37 does not decode
            // `then`: it rebuilds the key's statement without it and refuses the key.
            Self::PrivateReleases
            | Self::KeyFollowUps
            | Self::HeldTeamGrants
            | Self::InviteCreatorStanding
            | Self::BackupKeyApprovers => "1.0.0-rc.38",
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

/// What the gate ([`admit`]) decides for a proposal.
#[derive(Debug, PartialEq, Eq)]
pub enum Admission {
    /// The logged version brings every feature: propose it.
    Admitted,
    /// Every node supports the features now, but the logged version is below
    /// them: log this version first (`SetClusterVersion`), then judge again on
    /// the logged one. Proposed before it, an entry would be in the log while
    /// the version that keeps older nodes out (`may_join`) is not.
    LogFirst(String),
    /// Not every node supports them.
    Refused(NotActive),
}

/// The gate every proposal goes through (`Node::require_features`): on the
/// logged version, or, when every node supports the features already
/// ([`floor`]), on that version once it is logged. Only `SetClusterVersion`
/// itself (the entry that logs the floor), and a floor below the release that
/// brings it, are admitted on the floor alone.
#[must_use]
pub fn admit(logged: Option<&str>, floor: Option<&str>, features: &[Feature]) -> Admission {
    if check(logged, features).is_ok() {
        return Admission::Admitted;
    }
    let active = effective(logged, floor);
    if let Err(refused) = check(active.as_deref(), features) {
        return Admission::Refused(refused);
    }
    // Below rc.16 no node decodes `SetClusterVersion`, so nothing can be
    // logged: those features are judged on the floor alone, as before.
    let loggable = check(active.as_deref(), &[Feature::ClusterVersion]).is_ok();
    match active {
        Some(v) if loggable && features.iter().any(|f| *f != Feature::ClusterVersion) => {
            Admission::LogFirst(v)
        }
        _ => Admission::Admitted,
    }
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
) -> Result<Value, crate::system::Rejection> {
    if version.len() > MAX_RELEASE_VERSION_BYTES
        || semver::Version::parse(&version).is_err()
        || nodes.values().any(|v| v.len() > MAX_RELEASE_VERSION_BYTES)
    {
        return Err(BAD_VERSION.into());
    }
    let registered: Vec<u64> = s
        .replica_nodes()
        .map(|n| n.id)
        .filter(|id| !s.removed_nodes.contains(id))
        .collect();
    if registered.is_empty() || !nodes.keys().copied().eq(registered.iter().copied()) {
        return Err(NOT_EVERY_NODE.into());
    }
    if nodes
        .values()
        .any(|v| compare_versions(v, &version) == Ordering::Less)
    {
        return Err(ABOVE_A_NODE.into());
    }
    if s.cluster_version
        .as_deref()
        .is_some_and(|c| compare_versions(&version, c) != Ordering::Greater)
    {
        return Err(NOT_FORWARD.into());
    }
    s.cluster_version = Some(version.clone());
    Ok(json!({ "clusterVersion": version }))
}

/// Whether `class` carries an `unclassified` reason ([`Feature::UnclassifiedUpdates`]).
fn unclassified(class: &crate::types::UpgradeClass) -> bool {
    class
        .reasons
        .iter()
        .any(|r| matches!(r, calimero_bundle::update::Reason::Unclassified { .. }))
}

impl SystemCmd {
    /// The features this entry uses: none for a shape every 1.0 release
    /// applies alike. Every command is listed, so a new one decides here.
    #[must_use]
    pub fn features(&self) -> Vec<Feature> {
        match self {
            Self::SetClusterVersion { .. } => vec![Feature::ClusterVersion],
            Self::StartUpgradeRun {
                by_release,
                run,
                owner_approves,
                ..
            } => {
                let mut f = Vec::new();
                if *by_release {
                    f.push(Feature::PublishedRunByRelease);
                }
                if !run.targets.is_empty() {
                    f.push(Feature::UnitUpgrade);
                }
                if *owner_approves {
                    f.push(Feature::OwnerApprovedUpgrades);
                }
                f
            }
            Self::UpgradeApp {
                in_deployment,
                owner_approves,
                class,
                auto,
                members_stay,
                restore_point,
                ..
            } if *in_deployment
                || *owner_approves
                || class.is_some()
                || *auto
                || *members_stay
                || restore_point.is_some() =>
            {
                let mut f = Vec::new();
                if *in_deployment {
                    f.push(Feature::UpgradeInDeployment);
                }
                if *owner_approves {
                    f.push(Feature::OwnerApprovedUpgrades);
                }
                if class.is_some() {
                    f.push(Feature::UpgradeClasses);
                }
                if class.as_ref().is_some_and(unclassified) {
                    f.push(Feature::UnclassifiedUpdates);
                }
                if *auto {
                    f.push(Feature::AutoUpdates);
                }
                if *members_stay {
                    f.push(Feature::AutoUpdateMembersStay);
                }
                if restore_point.is_some() {
                    f.push(Feature::RestorePoints);
                }
                f
            }
            Self::UpgradeFailed {
                from_release: Some(_),
                ..
            } => vec![Feature::AutoUpdates],
            Self::ApproveUpgrade { class, .. } if unclassified(class) => {
                vec![Feature::UpgradePolicies, Feature::UnclassifiedUpdates]
            }
            Self::SetUpgradePolicy { .. } | Self::ApproveUpgrade { .. } => {
                vec![Feature::UpgradePolicies]
            }
            Self::UndoUpgrade {
                release_installed, ..
            } => {
                let mut f = vec![Feature::RestorePoints];
                if *release_installed {
                    f.push(Feature::UndoToInstalledRelease);
                }
                f
            }
            Self::CreateApiKey {
                owner_approves,
                then,
                ..
            } if *owner_approves || !then.is_empty() => {
                let mut f = Vec::new();
                if *owner_approves {
                    f.push(Feature::OwnerApprovedUpgrades);
                }
                if !then.is_empty() {
                    f.push(Feature::KeyFollowUps);
                }
                f
            }
            Self::MoveApp {
                accounts_in_app_team,
                org_quotas,
                ..
            } => {
                let mut f = vec![Feature::MoveApp];
                if *accounts_in_app_team {
                    f.push(Feature::MoveAccountsAppTeam);
                }
                if *org_quotas {
                    f.push(Feature::MoveOrgQuotas);
                }
                f
            }
            Self::SetOrgQuotas { .. } => vec![Feature::OrgQuotas],
            Self::SetOrgSettings { settings, .. } if settings.allow_public_links.is_some() => {
                let mut f = vec![Feature::PublicLinks];
                if settings.allow_guests.is_some() {
                    f.insert(0, Feature::Guests);
                }
                f
            }
            Self::CreatePublicLink { .. } | Self::RevokePublicLink { .. } => {
                vec![Feature::AccessGrants, Feature::PublicLinks]
            }
            Self::SetOrgSettings { .. } | Self::RevokeAppInvite { .. } => vec![Feature::Guests],
            Self::CreateAppInvite { .. } | Self::RedeemAppInvite { .. } => {
                vec![Feature::Guests, Feature::AccessGrants]
            }
            Self::SetAccess { .. } => vec![Feature::AccessGrants],
            Self::DeleteTeam {
                keep_apps: true, ..
            } => vec![Feature::KeepTeamApps],
            Self::SetDelegation { .. } => vec![Feature::Delegation],
            Self::LinkApp { .. } => vec![Feature::AttachApp],
            Self::SetDeploymentSecret { used: true, .. } => {
                vec![Feature::DeploymentSecrets, Feature::DeploymentUsedSecrets]
            }
            Self::SetDeploymentSecret { .. } | Self::ClearDeploymentSecret { .. } => {
                vec![Feature::DeploymentSecrets]
            }
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
            Self::RestoreApp { .. } | Self::FinishRestore { .. } => vec![Feature::RestoreApp],
            Self::RenameOrg { .. } => vec![Feature::RenameOrg],
            Self::AttributeByRelease => vec![Feature::AttributeByRelease],
            Self::RequireCurrentRules => vec![Feature::CurrentRules],
            Self::ExpireProofs => vec![Feature::ExpireProofs],
            Self::SetBackupKey { .. } | Self::ImportHolder { .. } => vec![Feature::BackupKeys],
            Self::SetTeamMembers { .. } | Self::RemoveTeamMembers { .. } => {
                vec![Feature::TeamMembersBatch]
            }
            Self::RedeemInvite {
                clamp_grants: true, ..
            }
            | Self::CreateAccount {
                invite:
                    Some(crate::system::SignUpRedemption {
                        clamp_grants: true, ..
                    }),
                ..
            } => vec![Feature::InviteCapabilities],
            Self::CreateAccount { .. } => Vec::new(),
            Self::RedeemInvite { .. } => Vec::new(),
            Self::CreateInvite { invite, .. } => {
                invite_features(invite.all_apps, invite.capabilities.is_some())
            }
            Self::SetPriceTable { .. } => vec![Feature::PriceTable],
            Self::InstallPackage { release, .. } => {
                let mut f = Vec::new();
                if !release.components.is_app() {
                    f.push(Feature::ComputeOnly);
                }
                if !release.pipelines.is_empty() {
                    f.push(Feature::Pipelines);
                }
                if !release.secrets.is_empty() {
                    f.push(Feature::DeclaredSecrets);
                }
                if !release.uses.is_empty() {
                    f.push(Feature::DeploymentUsedSecrets);
                }
                if release.instance_org.is_some() {
                    f.push(Feature::OrdinaryOrgs);
                }
                if release.guests.is_some() {
                    f.push(Feature::Guests);
                }
                if release.visibility.is_some() || !release.orgs.is_empty() {
                    f.push(Feature::PrivateReleases);
                }
                f
            }
            Self::CreateApp {
                app,
                unit_offer,
                offers_only,
            } => {
                let mut f = Vec::new();
                if !app.components.is_app() {
                    f.push(Feature::ComputeOnly);
                }
                if !app.links.is_empty() {
                    f.push(Feature::AppLinks);
                }
                if unit_offer.is_some() {
                    f.push(Feature::WholeUnits);
                }
                if !app.access.grants().is_empty() {
                    f.push(Feature::AccessGrants);
                }
                if *offers_only {
                    f.push(Feature::OffersOnly);
                }
                f
            }
            Self::DeleteApp {
                members: Some(_), ..
            } => vec![Feature::WholeUnits],
            Self::AcquireRun { pool: Some(_), .. } => vec![Feature::NamedPool],
            Self::CreateOrg {
                app_team,
                capabilities,
                ..
            } => {
                let mut f = Vec::new();
                if app_team.is_some() {
                    f.push(Feature::OrgAppTeam);
                }
                if capabilities.is_some() {
                    f.push(Feature::OrgCapabilities);
                }
                f
            }
            Self::RegisterNode { .. }
            | Self::RemoveNode { .. }
            | Self::PutBundle { .. }
            | Self::UninstallPackage { .. }
            | Self::UpgradeApp { .. }
            | Self::TransferApp { .. }
            | Self::DeleteApp { members: None, .. }
            | Self::ChargeFileBytes { .. }
            | Self::AcquireRun { .. }
            | Self::ReleaseRun { .. }
            | Self::SetReplicas { .. }
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
            | Self::SetEgress { .. }
            | Self::DeleteOrg { .. }
            | Self::SetOrgMetadata { .. }
            | Self::SetMemberMetadata { .. }
            | Self::SetAppMetadata { .. }
            | Self::SetPasskeyLabel { .. }
            | Self::UpgradeFailed { .. }
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

/// The features an invite uses: `allApps` and `capabilities` are dropped by
/// an earlier release, so the route and the consent page ask for them first.
#[must_use]
pub fn invite_features(all_apps: bool, capabilities: bool) -> Vec<Feature> {
    let mut f = Vec::new();
    if all_apps {
        f.push(Feature::AllApps);
    }
    if capabilities {
        f.push(Feature::InviteCapabilities);
    }
    f
}

/// The features a `ScopeApps` with `apps` uses: one app team's `join` or
/// `keep` ([`Feature::AppTeamJoin`]), its `childTeams`
/// ([`Feature::AppTeamChildTeams`]).
#[must_use]
pub fn scope_app_features(apps: Option<&[ScopeApp]>) -> Vec<Feature> {
    let apps = apps.unwrap_or_default();
    let mut f = Vec::new();
    if apps.iter().any(|a| a.join.is_some() || a.keep.is_some()) {
        f.push(Feature::AppTeamJoin);
    }
    if apps.iter().any(|a| a.child_teams.is_some()) {
        f.push(Feature::AppTeamChildTeams);
    }
    f
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use serde::Deserialize;
    use serde_json::{json, Value};

    use super::{
        admit, advertised, check, effective, floor, may_join, supported, Admission, Feature,
        ABOVE_A_NODE, FEATURES, NOT_EVERY_NODE, NOT_FORWARD,
    };
    use crate::system::{compare_versions, OrgQuotas, ScopeApp, SystemCmd, SystemMachine};
    use crate::types::TeamJoin;

    const RC14: &str = "1.0.0-rc.14";
    const RC15: &str = "1.0.0-rc.15";
    const RC16: &str = "1.0.0-rc.16";
    const RC19: &str = "1.0.0-rc.19";
    const RC20: &str = "1.0.0-rc.20";
    const RC22: &str = "1.0.0-rc.22";
    const RC24: &str = "1.0.0-rc.24";
    const RC29: &str = "1.0.0-rc.29";
    const RC31: &str = "1.0.0-rc.31";
    const RC32: &str = "1.0.0-rc.32";
    const RC33: &str = "1.0.0-rc.33";
    const RC34: &str = "1.0.0-rc.34";
    const RC35: &str = "1.0.0-rc.35";
    const RC36: &str = "1.0.0-rc.36";
    const RC37: &str = "1.0.0-rc.37";
    const RC38: &str = "1.0.0-rc.38";

    /// An entry that uses `feature`, as a node proposes it.
    fn uses(feature: Feature) -> SystemCmd {
        match feature {
            Feature::Delegation => entry(json!({ "op": "setDelegation", "account": "a",
                "source": "app", "target": "kv-app", "methods": ["get"] })),
            Feature::InviteCapabilities => SystemCmd::CreateInvite {
                invite_key: "k".into(),
                invite: serde_json::from_value(json!({
                    "org": "org", "role": "member", "expiresAt": 1, "usesLeft": 1,
                    "capabilities": 2,
                }))
                .unwrap(),
                actor: "alice".into(),
                consent: Value::Null,
                teams_seen: true,
            },
            Feature::OrgCapabilities => SystemCmd::CreateOrg {
                org_id: "org-d".into(),
                name: "Chat".into(),
                actor: "alice".into(),
                release: None,
                recovery_key: None,
                at: 0,
                name_fits: true,
                personal: false,
                app_team: None,
                capabilities: Some(crate::types::caps::INVITE_MEMBERS),
            },
            Feature::OrgAppTeam => SystemCmd::CreateOrg {
                org_id: "org-c".into(),
                name: "Chat".into(),
                actor: "alice".into(),
                release: None,
                recovery_key: None,
                at: 0,
                name_fits: true,
                personal: false,
                app_team: Some("chat".into()),
                capabilities: None,
            },
            Feature::BackupKeys => SystemCmd::SetBackupKey {
                holder: crate::system::BackupHolder::Operator,
                public_key: "ab".repeat(32),
                actor: "operator".into(),
                at: 0,
                proof: Value::Null,
            },
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
                org_quotas: false,
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
                org_quotas: false,
            },
            Feature::MoveOrgQuotas => SystemCmd::MoveApp {
                app_id: "app".into(),
                to_org: "org-b".into(),
                actor: "alice".into(),
                consent: Value::Null,
                bring_members: false,
                nonce: "n".into(),
                accounts_in_app_team: false,
                all_apps_rows: false,
                org_quotas: true,
            },
            Feature::PublishedRunByRelease => SystemCmd::StartUpgradeRun {
                org: "org".into(),
                run: Box::new(
                    serde_json::from_value(serde_json::json!({
                        "runId": "r", "package": "p", "version": "1.0.0", "by": "alice",
                        "startedAt": 0, "apps": [], "done": [], "failed": {}, "finished": false,
                    }))
                    .unwrap(),
                ),
                consent: Value::Null,
                by_release: true,
                owner_approves: false,
            },
            Feature::AppTeamJoin => SystemCmd::ScopeApps {
                org: "org".into(),
                apps: Some(vec![ScopeApp {
                    package: "chat".into(),
                    name: None,
                    instances: None,
                    join: Some(TeamJoin::Invite),
                    keep: None,
                    child_teams: None,
                }]),
                shared: None,
                nonce: "n".into(),
                actor: "alice".into(),
                consent: Value::Null,
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
                member_child_teams: true,
                by_release: true,
                all_apps_rows: false,
            },
            Feature::AppTeamChildTeams => SystemCmd::ScopeApps {
                org: "org".into(),
                apps: Some(vec![ScopeApp {
                    package: "chat".into(),
                    name: None,
                    instances: Some(vec![]),
                    join: None,
                    keep: None,
                    child_teams: Some(true),
                }]),
                shared: Some(false),
                nonce: "n".into(),
                actor: "alice".into(),
                consent: Value::Null,
                member_child_teams: true,
                by_release: false,
                all_apps_rows: false,
            },
            Feature::TeamMembersBatch => SystemCmd::RemoveTeamMembers {
                org: "org".into(),
                account: "bob".into(),
                teams: vec!["team".into()],
                actor: "alice".into(),
                consent: Value::Null,
            },
            Feature::RenameOrg => SystemCmd::RenameOrg {
                org: "org".into(),
                name: "Renamed".into(),
                actor: "alice".into(),
                consent: Value::Null,
            },
            Feature::AttributeByRelease => SystemCmd::AttributeByRelease,
            Feature::CurrentRules => SystemCmd::RequireCurrentRules,
            Feature::ExpireProofs => SystemCmd::ExpireProofs,
            Feature::RestoreApp => SystemCmd::FinishRestore {
                app_id: "app".into(),
                group: 2,
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
            Feature::PriceTable => entry(json!({ "op": "setPriceTable", "table": {
                "version": 1, "fuelPerVcpuSecond": 1, "vcpuSecond": 1, "gibSecond": 1,
                "connectorGib": 1, "storageGibMonth": 1, "egressGib": 1,
            } })),
            Feature::ComputeOnly => install(json!({ "components": "computeOnly" })),
            Feature::UnitUpgrade => entry(json!({ "op": "startUpgradeRun", "org": "org",
                "run": { "runId": "r", "package": "p", "version": "2.0.0", "by": "a",
                    "startedAt": 1, "apps": [], "done": [], "failed": {}, "finished": false,
                    "targets": { "kv-app": { "package": "kv", "version": "0.2.0" } } } })),
            Feature::AppLinks => entry(json!({ "op": "createApp", "app": {
                "appId": "app", "name": "notes", "group": 0, "bundle": "b", "owner": "a",
                "org": "org", "confirmMethods": [], "replicas": [1],
                "createdAt": 1, "initArgs": null, "metadata": null,
                "links": { "kv": "kv-app" },
            } })),
            Feature::AttachApp => entry(json!({ "op": "linkApp", "app_id": "app", "name": "kv",
                "target": "kv-app", "actor": "a" })),
            Feature::WholeUnits => entry(json!({ "op": "deleteApp", "app_id": "app",
                "actor": "a", "members": [] })),
            Feature::DeploymentSecrets => entry(json!({ "op": "setDeploymentSecret",
                "deployment": "d", "name": "API_KEY", "value": "v", "actor": "a", "at": 1 })),
            Feature::DeploymentUsedSecrets => install(json!({
                "uses": { "inference": { "package": "pi", "range": "^1" } },
            })),
            Feature::UpgradeInDeployment => entry(json!({ "op": "upgradeApp",
                "app_id": "app", "bundle": "b", "actor": "a", "in_deployment": true })),
            Feature::OrdinaryOrgs => install(json!({ "instanceOrg": "standalone" })),
            Feature::Guests => entry(json!({ "op": "setOrgSettings", "org": "org",
                "settings": { "allowGuests": false }, "actor": "alice" })),
            Feature::AccessGrants => entry(json!({ "op": "setAccess", "app_id": "app",
                "grants": [{ "kind": "team", "team": "t" },
                    { "kind": "account", "account": "carol", "scope": { "channel": "p" } }],
                "actor": "alice" })),
            Feature::KeepTeamApps => entry(json!({ "op": "deleteTeam", "org": "org",
                "team": "t", "actor": "alice", "apps_need_consent": true, "keep_apps": true })),
            Feature::OwnerApprovedUpgrades => entry(json!({ "op": "upgradeApp",
                "app_id": "app", "bundle": "b", "actor": "a", "owner_approves": true })),
            Feature::PublicLinks => entry(json!({ "op": "setOrgSettings", "org": "org",
                "settings": { "allowPublicLinks": false }, "actor": "alice" })),
            Feature::UpgradeClasses => entry(json!({ "op": "upgradeApp",
                "app_id": "app", "bundle": "b", "actor": "a",
                "class": { "class": "green" } })),
            Feature::UpgradePolicies => entry(json!({ "op": "setUpgradePolicy",
                "org": "org", "policy": { "secondApprover": true }, "actor": "alice" })),
            Feature::AutoUpdates => entry(json!({ "op": "upgradeApp",
                "app_id": "app", "bundle": "b", "actor": "policy:auto-update", "auto": true })),
            Feature::RestorePoints => entry(json!({ "op": "undoUpgrade",
                "app_id": "app", "apps": { "app": [1] }, "actor": "alice", "at": 1 })),
            Feature::OffersOnly => entry(json!({ "op": "createApp", "app": {
                "appId": "app", "name": "notes", "group": 0, "bundle": "b", "owner": "a",
                "org": "org", "confirmMethods": [], "replicas": [1],
                "createdAt": 1, "initArgs": null, "metadata": null,
            }, "offers_only": true })),
            Feature::UndoToInstalledRelease => entry(json!({ "op": "undoUpgrade",
                "app_id": "app", "apps": { "app": [1] }, "actor": "alice", "at": 1,
                "release_installed": true })),
            Feature::AutoUpdateMembersStay => entry(json!({ "op": "upgradeApp",
                "app_id": "app", "bundle": "b", "actor": "policy:auto-update", "auto": true,
                "members_stay": true })),
            Feature::KeyFollowUps => entry(json!({ "op": "createApiKey", "hash": "h",
                "record": { "keyId": "k", "account": "a", "label": "", "expiresAt": 1 },
                "then": [{ "app": "app", "method": "setAccess", "argsHash": "h" }] })),
            Feature::UnclassifiedUpdates => entry(json!({ "op": "approveUpgrade",
                "app_id": "app", "package": "p", "version": "2.0.0", "actor": "alice", "at": 1,
                "class": { "class": "red", "reasons": [{ "code": "unclassified",
                    "member": null, "package": "p", "version": "1.0.0" }] } })),
            Feature::PrivateReleases => {
                install(json!({ "visibility": "private", "orgs": ["org"] }))
            }
            Feature::Pipelines => install(json!({
                "pipelines": { "p1": { "hash": "h", "worstCaseFuel": "1" } },
            })),
            Feature::DeclaredSecrets => install(json!({
                "outbound": ["https://api.example.com/"],
                "secrets": { "API_KEY": { "host": "api.example.com", "paths": ["/"], "header": "x-api-key" } },
            })),
            Feature::HeldTeamGrants
            | Feature::InviteCreatorStanding
            | Feature::BackupKeyApprovers => unreachable!("no entry names {feature:?}"),
            Feature::NamedPool => entry(json!({
                "op": "acquireRun", "app": "app", "run": "r", "payer": "p", "fuel": 1,
                "memory_mib": 1, "pool": "big",
            })),
        }
    }

    fn entry(v: Value) -> SystemCmd {
        serde_json::from_value(v).unwrap()
    }

    /// An `InstallPackage` of a plain release with `extra` fields set.
    fn install(extra: Value) -> SystemCmd {
        let mut release = json!({
            "version": "1.0.0", "mpk": "h", "mpkSize": 1, "source": "upload",
            "services": {}, "installedBy": "a", "installedAt": 1,
        });
        release
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        entry(
            json!({ "op": "installPackage", "package": "p", "signer_id": "s", "release": release }),
        )
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
        fn apply(&mut self, cmd: &SystemCmd) -> Result<Value, crate::system::Rejection> {
            if check(Some(self.release), &cmd.features()).is_err() {
                return Err("malformed command".into());
            }
            self.machine.run_authorized(cmd.clone())
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
    /// (`Node::require_features`, [`admit`]): the logged version, or what
    /// every node advertises now, logged first. Admitted, every node applies it.
    fn propose(nodes: &mut [Replica], cmd: &SystemCmd) -> Result<(), super::NotActive> {
        let logged = nodes[0].machine.state.cluster_version.clone();
        let low = floor(nodes.iter().map(|n| Some(n.release)));
        match admit(logged.as_deref(), low.as_deref(), &cmd.features()) {
            Admission::Admitted => {}
            Admission::Refused(refused) => return Err(refused),
            Admission::LogFirst(version) => {
                let raise = SystemCmd::SetClusterVersion {
                    version,
                    nodes: (1..)
                        .zip(nodes.iter().map(|n| n.release.to_owned()))
                        .collect(),
                };
                propose(nodes, &raise)?;
            }
        }
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
        let answers: Vec<Result<Value, crate::system::Rejection>> =
            nodes.iter_mut().map(|n| n.apply(&cmd)).collect();
        assert!(answers[0].is_ok() && answers[1].is_ok(), "{answers:?}");
        assert_eq!(answers[2], Err("malformed command".into()));
        assert!(
            !same_state(&nodes),
            "the older node kept the state before it"
        );
    }

    /// A feature's entry is proposed only after the version is logged. While every node
    /// runs rc.16 and no version is logged yet, the gate admits a feature of rc.16 on what
    /// the nodes advertise; were the entry proposed then, a node on rc.15 registered before
    /// the leader logs rc.16 would still be admitted at the logged version (`may_join`),
    /// and reach an entry it cannot decode.
    #[test]
    fn a_feature_is_proposed_only_once_the_version_that_keeps_older_nodes_out_is_logged() {
        let mut nodes = cluster(&[RC16, RC16, RC16]);
        let feature = FEATURES
            .iter()
            .copied()
            .find(|f| f.since() == RC16 && *f != Feature::ClusterVersion)
            .unwrap();
        propose(&mut nodes, &uses(feature)).unwrap();
        let logged = nodes[0].machine.state.cluster_version.clone();
        assert!(
            !may_join(logged.as_deref(), Some(RC15)),
            "an rc.15 node would join a log holding {feature:?} at the logged version {logged:?}"
        );
        assert_eq!(logged.as_deref(), Some(RC16));
        assert!(same_state(&nodes));
    }

    /// `SetClusterVersion` is the one entry admitted on what the nodes
    /// advertise alone: it is what logs it.
    #[test]
    fn only_the_cluster_version_itself_is_admitted_before_it_is_logged() {
        let raise = [Feature::ClusterVersion];
        assert_eq!(admit(None, Some(RC16), &raise), Admission::Admitted);
        assert_eq!(
            admit(None, Some(RC16), &[Feature::AttachApp]),
            Admission::Refused(check(Some(RC16), &[Feature::AttachApp]).unwrap_err())
        );
        let quotas = [Feature::OrgQuotas];
        assert_eq!(
            admit(Some(RC14), Some(RC16), &quotas),
            Admission::LogFirst(RC16.into())
        );
        assert_eq!(admit(Some(RC16), Some(RC14), &quotas), Admission::Admitted);
        assert!(matches!(
            admit(Some(RC14), Some(RC14), &quotas),
            Admission::Refused(_)
        ));
        assert!(matches!(
            admit(Some(RC14), None, &quotas),
            Admission::Refused(_)
        ));
        // Below rc.16 nothing decodes `SetClusterVersion`: the floor alone.
        assert_eq!(admit(None, Some(RC15), &quotas), Admission::Admitted);
    }

    /// A system entry this node cannot decode (a later release's command) fails
    /// the group's batch, which stops its Raft core on that entry: skipped, the
    /// node would serve a state the others no longer have (a revoked API key
    /// still working there).
    #[test]
    fn an_entry_this_node_cannot_decode_stops_the_system_group() {
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
        let mut m = SystemMachine::default();
        let before = serde_json::to_value(&m.state).unwrap();
        let mut tx = calimero_log::StateTx::new(&group);
        let later = br#"{"op":"revokeApiKeyOfALaterRelease","keyId":"k"}"#;
        let _answer = calimero_log::Machine::apply(&mut m, 7, later, &mut tx);
        assert!(
            tx.commit().is_err(),
            "the batch committed: the node skipped the entry and goes on"
        );
        assert_eq!(serde_json::to_value(&m.state).unwrap(), before);
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
        for feature in FEATURES.into_iter().filter(|f| f.names_an_entry()) {
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
        let (older, newer): (Vec<Feature>, Vec<Feature>) = FEATURES
            .iter()
            .filter(|f| f.names_an_entry())
            .partition(|f| {
                ![
                    RC19, RC20, RC22, RC24, RC29, RC31, RC32, RC33, RC34, RC35, RC36, RC38,
                ]
                .contains(&f.since())
            });
        let (engine, later): (Vec<Feature>, Vec<Feature>) =
            newer.into_iter().partition(|f| f.since() == RC19);
        let (after, last): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC20);
        for feature in older {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        for &feature in engine.iter().chain(&after) {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC16))
            );
        }
        assert!(same_state(&nodes));
        assert!(nodes[0].machine.state.org_quotas.contains_key("org"));

        for n in &mut nodes {
            n.release = RC19;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC19.into(),
            nodes: (1..=3).map(|id| (id, RC19.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &engine {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.20's wait for every node on rc.20.
        for &feature in &after {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC19))
            );
        }
        for n in &mut nodes {
            n.release = RC20;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC20.into(),
            nodes: (1..=3).map(|id| (id, RC20.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &after {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.22's wait for every node on rc.22 (an entry naming several is refused for the
        // first it needs: an unclassified approval for `upgradePolicies`, an undo with the
        // installed-release rule for `restorePoints`).
        for &feature in &last {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            let first = match feature {
                Feature::UnclassifiedUpdates => Feature::UpgradePolicies,
                Feature::UndoToInstalledRelease => Feature::RestorePoints,
                Feature::AutoUpdateMembersStay => Feature::AutoUpdates,
                _ => feature,
            };
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (first, Some(RC20))
            );
        }
        for n in &mut nodes {
            n.release = RC22;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC22.into(),
            nodes: (1..=3).map(|id| (id, RC22.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        // rc.24's and rc.29's wait for every node on them.
        let (rc22, later): (Vec<Feature>, Vec<Feature>) =
            last.into_iter().partition(|f| f.since() == RC22);
        let (rc24, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC24);
        let (rc29, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC29);
        let (rc30, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC31);
        let (rc32, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC32);
        let (rc33, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC33);
        let (rc34, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC34);
        let (rc35, later): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC35);
        let (rc36, rc38): (Vec<Feature>, Vec<Feature>) =
            later.into_iter().partition(|f| f.since() == RC36);
        for &feature in &rc22 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        for &feature in rc24
            .iter()
            .chain(&rc29)
            .chain(&rc30)
            .chain(&rc32)
            .chain(&rc33)
        {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC22))
            );
        }
        for n in &mut nodes {
            n.release = RC24;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC24.into(),
            nodes: (1..=3).map(|id| (id, RC24.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc24 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        for &feature in rc29.iter().chain(&rc30).chain(&rc32).chain(&rc33) {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC24))
            );
        }
        for n in &mut nodes {
            n.release = RC29;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC29.into(),
            nodes: (1..=3).map(|id| (id, RC29.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc29 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.31's wait for every node on rc.31.
        for &feature in rc30.iter().chain(&rc32).chain(&rc33) {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC29))
            );
        }
        for n in &mut nodes {
            n.release = RC31;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC31.into(),
            nodes: (1..=3).map(|id| (id, RC31.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc30 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.32's (a deployment's secrets, whole units, secrets of used releases, an
        // upgrade kept on its deployment's offer, a release's instance mode) wait for every
        // node on rc.32, and rc.33's (upgrades only an organization's owner and admins
        // approve, guests, access grants, a team deleted keeping its instances, public
        // links) for every node on rc.33.
        assert_eq!(
            rc32,
            [
                Feature::DeploymentSecrets,
                Feature::WholeUnits,
                Feature::DeploymentUsedSecrets,
                Feature::UpgradeInDeployment,
                Feature::OrdinaryOrgs,
            ]
        );
        assert_eq!(
            rc33,
            [
                Feature::OwnerApprovedUpgrades,
                Feature::Guests,
                Feature::AccessGrants,
                Feature::KeepTeamApps,
                Feature::PublicLinks
            ]
        );
        // rc.34's (update classes and policies) for every node on rc.34.
        assert_eq!(
            rc34,
            [
                Feature::UpgradeClasses,
                Feature::UpgradePolicies,
                Feature::AutoUpdates,
                Feature::RestorePoints
            ]
        );
        for &feature in rc32.iter().chain(&rc33) {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC31))
            );
        }
        let clear = entry(json!({ "op": "clearDeploymentSecret", "deployment": "d",
            "name": "API_KEY", "actor": "a" }));
        assert_eq!(
            propose(&mut nodes, &clear).unwrap_err().feature,
            Feature::DeploymentSecrets
        );
        // A name a release an offered one references declares: both rc.32 features.
        let used = entry(json!({ "op": "setDeploymentSecret", "deployment": "d",
            "name": "API_KEY", "value": "v", "actor": "a", "at": 1, "used": true }));
        assert_eq!(
            used.features(),
            [Feature::DeploymentSecrets, Feature::DeploymentUsedSecrets]
        );
        assert!(propose(&mut nodes, &used).is_err());
        for n in &mut nodes {
            n.release = RC32;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC32.into(),
            nodes: (1..=3).map(|id| (id, RC32.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc32 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        for &feature in &rc33 {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC32))
            );
        }
        for n in &mut nodes {
            n.release = RC33;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC33.into(),
            nodes: (1..=3).map(|id| (id, RC33.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc33 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        for &feature in &rc34 {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC33))
            );
        }
        for n in &mut nodes {
            n.release = RC34;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC34.into(),
            nodes: (1..=3).map(|id| (id, RC34.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc34 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.35's (an update the log cannot classify, approved and applied as red) for
        // every node on rc.35: an rc.34 node cannot decode the `unclassified` reason.
        assert_eq!(rc35, [Feature::UnclassifiedUpdates]);
        for &feature in &rc35 {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC34))
            );
            assert!(refused.message().contains(RC35), "{}", refused.message());
        }
        for n in &mut nodes {
            n.release = RC35;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC35.into(),
            nodes: (1..=3).map(|id| (id, RC35.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc35 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.36's (an instance outside every deployment is a deployer's, an undo only to a
        // release still installed, an update on its own that keeps the unit's members) for
        // every node on rc.36: an rc.35 node ignores the flags.
        assert_eq!(
            rc36,
            [
                Feature::OffersOnly,
                Feature::UndoToInstalledRelease,
                Feature::AutoUpdateMembersStay,
            ]
        );
        for &feature in &rc36 {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC35))
            );
        }
        for n in &mut nodes {
            n.release = RC36;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC36.into(),
            nodes: (1..=3).map(|id| (id, RC36.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc36 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        // rc.38's (who may use a release, a key with follow-ups) waits past rc.37, which was cut without it: a
        // cluster on the published rc.37 logs rc.37 and still refuses it, as an rc.37 node
        // would refuse the key (it rebuilds the statement without `then`).
        assert_eq!(rc38, [Feature::PrivateReleases, Feature::KeyFollowUps]);
        for n in &mut nodes {
            n.release = RC37;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC37.into(),
            nodes: (1..=3).map(|id| (id, RC37.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc38 {
            let refused = propose(&mut nodes, &uses(feature)).unwrap_err();
            assert_eq!(
                (refused.feature, refused.active.as_deref()),
                (feature, Some(RC37))
            );
        }
        for n in &mut nodes {
            n.release = RC38;
        }
        let raise = SystemCmd::SetClusterVersion {
            version: RC38.into(),
            nodes: (1..=3).map(|id| (id, RC38.to_owned())).collect(),
        };
        propose(&mut nodes, &raise).unwrap();
        for &feature in &rc38 {
            propose(&mut nodes, &uses(feature)).unwrap();
        }
        assert!(same_state(&nodes));
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
            Feature::RestoreApp,
            Feature::RenameOrg,
            Feature::TeamMembersBatch,
            Feature::AppTeamChildTeams,
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
        assert_eq!(
            m.run_authorized(set(RC16, &two)),
            Err(NOT_EVERY_NODE.into())
        );
        let four = [(1, RC16), (2, RC16), (3, RC16), (4, RC16)];
        assert_eq!(
            m.run_authorized(set(RC16, &four)),
            Err(NOT_EVERY_NODE.into())
        );
        // Never above what a node supports.
        let behind = [(1, RC16), (2, RC16), (3, RC15)];
        assert_eq!(
            m.run_authorized(set(RC16, &behind)),
            Err(ABOVE_A_NODE.into())
        );
        assert!(m.run_authorized(set("not a version", &behind)).is_err());
        let _ok = m.run_authorized(set(RC15, &behind)).unwrap();
        assert_eq!(m.state.cluster_version.as_deref(), Some(RC15));
        let all = [(1, RC16), (2, RC16), (3, RC16)];
        let _ok = m.run_authorized(set(RC16, &all)).unwrap();
        // Never back, never the same again.
        assert_eq!(m.run_authorized(set(RC16, &all)), Err(NOT_FORWARD.into()));
        assert_eq!(m.run_authorized(set(RC15, &all)), Err(NOT_FORWARD.into()));
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

    /// Entries an earlier release would misread name their features only when they use them.
    #[test]
    fn compute_only_pipeline_pool_and_price_entries_name_their_features() {
        assert!(install(json!({})).features().is_empty());
        for f in [
            Feature::PriceTable,
            Feature::ComputeOnly,
            Feature::Pipelines,
            Feature::NamedPool,
        ] {
            assert_eq!(uses(f).features(), [f]);
            assert_eq!(f.since(), RC19, "rc.18 was cut without the pipeline engine");
        }
        let mut run = json!({
            "op": "acquireRun", "app": "app", "run": "r", "payer": "p", "fuel": 1, "memory_mib": 1,
        });
        assert!(entry(run.clone()).features().is_empty());
        run["pool"] = json!("big");
        assert_eq!(entry(run).features(), [Feature::NamedPool]);
    }

    /// An invite is an older shape unless it carries capabilities, which an
    /// earlier release would drop.
    #[test]
    fn only_an_invite_carrying_capabilities_needs_invite_capabilities() {
        let mut plain = uses(Feature::InviteCapabilities);
        if let SystemCmd::CreateInvite { invite, .. } = &mut plain {
            invite.capabilities = None;
        }
        assert!(plain.features().is_empty());
        assert_eq!(
            uses(Feature::InviteCapabilities).features(),
            [Feature::InviteCapabilities]
        );
    }

    /// An invite asks only for the features its fields use, and an older
    /// cluster refuses each.
    #[test]
    fn an_invites_fields_name_their_features() {
        use super::invite_features;

        assert!(invite_features(false, false).is_empty());
        assert_eq!(invite_features(true, false), [Feature::AllApps]);
        assert_eq!(invite_features(false, true), [Feature::InviteCapabilities]);
        for (all_apps, capabilities, feature, below) in [
            (true, false, Feature::AllApps, RC15),
            (false, true, Feature::InviteCapabilities, RC24),
        ] {
            assert_eq!(
                compare_versions(below, feature.since()),
                std::cmp::Ordering::Less
            );
            let refused = check(Some(below), &invite_features(all_apps, capabilities)).unwrap_err();
            assert_eq!(refused.feature, feature);
        }
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
