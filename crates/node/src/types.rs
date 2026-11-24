//! Types shared by the node's state machines and its API.

use std::collections::BTreeMap;

use calimero_identity::Credential;
use serde::{Deserialize, Serialize};

/// An account id: 32 random bytes, base64url.
pub type AccountId = String;
/// An app instance id: 16 random bytes, base64url.
pub type AppId = String;
/// An organization id: 16 random bytes, base64url.
pub type OrgId = String;
/// A team id: 16 random bytes, base64url.
pub type TeamId = String;

/// Who made a call.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Caller {
    /// A person, signed in with a passkey.
    Account { account: AccountId },
    /// A machine, with an API key of `account`.
    ApiKey { key: String, account: AccountId },
    /// A caller without an account (no access grant admits one).
    Public,
    /// The app itself: an `xcall` from app `app` (then the caller is the calling
    /// app). Never from a client: the API builds callers from credentials.
    App { app: AppId },
    /// The platform itself: a component's timer ticks (D34), which reach only
    /// methods whose ABI says `allowSystem`. Never from a client.
    System,
    /// An anonymous visitor through one of the instance's public links
    /// (a [`Grant::Link`] of the instance): the link's key and the visitor's Ed25519 public key
    /// (hex), which it proved it holds. It reaches only `#[app::public]` methods of that
    /// instance. Written in an app's log only from the cluster version that brings
    /// `Feature::PublicLinks`, so every replica decodes it.
    Visitor { link: String, key: String },
}

impl Caller {
    #[must_use]
    pub fn account(&self) -> Option<&str> {
        match self {
            Self::Account { account } | Self::ApiKey { account, .. } => Some(account),
            Self::Public | Self::App { .. } | Self::System | Self::Visitor { .. } => None,
        }
    }

    /// The caller as 32 bytes: an account's id, an app's context id.
    #[must_use]
    pub fn account_bytes(&self) -> [u8; 32] {
        // An app calls as its context id (`SHA-256(app_id)`, the id SDK 0.12 apps saw).
        if let Self::App { app } = self {
            return calimero_identity::sha256(app.as_bytes());
        }
        let mut out = [0; 32];
        if let Some(a) = self.account() {
            if let Ok(b) = calimero_identity::from_b64url(a) {
                let n = b.len().min(32);
                out[..n].copy_from_slice(&b[..n]);
            }
        }
        out
    }

    /// Whether the caller is a person: a signed-in account, or an API key acting for one.
    /// Only a person is carried to another app as the one a run serves (`on_behalf_of`).
    #[must_use]
    pub const fn is_person(&self) -> bool {
        matches!(self, Self::Account { .. } | Self::ApiKey { .. })
    }

    /// The caller as the component host core sees it (the WIT `caller`, D13).
    #[must_use]
    pub fn to_host(&self) -> calimero_app_host::Caller {
        use calimero_app_host::Caller as H;
        match self {
            Self::Account { account } => H::Account(account.clone()),
            Self::ApiKey { key, account } => H::ApiKey {
                key_id: key.clone(),
                account: account.clone(),
            },
            Self::Public => H::Public,
            Self::App { app } => H::App(app.clone()),
            Self::System => H::System,
            Self::Visitor { link, key } => H::Visitor {
                link: link.clone(),
                key: key.clone(),
            },
        }
    }
}

/// Who may reach an app at all: the instance's grants, each a way in
/// ([`crate::system::SystemState::may_reach`]). None: the owner alone. Finer rules are the
/// app's own code. Serialized as the list itself (an app record's `grants`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Access(pub Vec<Grant>);

/// One way into an instance ([`Access`]).
///
/// Member grants (the whole organization, or teams of it) admit its members; an account grant
/// names one person, a member (a named member) or not (a guest, admitted only while the
/// instance's release and organization allow guests,
/// [`crate::system::SystemState::guests_allowed`]); [`Self::Anyone`] admits every signed-in
/// account. Nobody without an account is admitted by a grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase", deny_unknown_fields)]
pub enum Grant {
    /// Every member of the instance's organization (an `app_only` guest of the
    /// organization only through its teams). Not beside a team grant.
    Org,
    /// Members of a team of the instance's organization: its direct members, and, for an
    /// open team, everyone who reaches its parent.
    Team { team: TeamId },
    /// One account: a member of the organization (a named member) or a guest.
    Account {
        account: AccountId,
        /// What the app lets this person do inside it, in the app's own terms (for a chat:
        /// `{"channel":"partners"}`): opaque JSON of at most [`MAX_GRANT_SCOPE_BYTES`], which
        /// the platform carries and never reads. Absent: the app's whole.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        scope: Option<serde_json::Value>,
    },
    /// Any signed-in account, a member of the organization or not. Never a caller without an
    /// account: those come in only through a public link, which admits the holders of its
    /// token and only to public methods.
    Anyone,
    /// A public link (`Feature::PublicLinks`): the holders of its token call the instance's
    /// `#[app::public]` methods as anonymous visitors (`Caller::Visitor`), nothing else. It
    /// admits no account ([`crate::system::SystemState::may_reach`]); visitors are judged by
    /// [`crate::system::SystemState::public_link_admits`]. Made and revoked only with their
    /// own consent (`CreatePublicLink`, `RevokePublicLink`), never by `SetAccess`, which keeps
    /// the instance's links as they are.
    Link(LinkGrant),
}

/// A public link of an instance, as its grant list keeps it ([`Grant::Link`]): under the
/// token's Ed25519 public key ([`crate::proof::public_link_key`]), so a node that only saw the
/// log cannot use it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LinkGrant {
    /// The token's public key, hex.
    pub link: String,
    /// A name its managers know it by (at most `MAX_PUBLIC_LINK_LABEL_CHARS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// When it stops admitting visitors (Unix seconds); none: until revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Calls one visitor may make through it a minute, counted on each node
    /// (`api::VisitorLimits`).
    pub rate_per_minute: u32,
    /// Who made it.
    #[serde(default)]
    pub created_by: AccountId,
    /// When it was made (Unix seconds, the creating node's clock).
    #[serde(default)]
    pub created_at: u64,
}

impl LinkGrant {
    /// The grant `link`, made under `key`, is kept as on its instance.
    #[must_use]
    pub fn of(key: &str, link: PublicLink) -> Self {
        Self {
            link: key.to_owned(),
            label: link.label,
            expires_at: link.expires_at,
            rate_per_minute: link.rate_per_minute,
            created_by: link.created_by,
            created_at: link.created_at,
        }
    }
}

/// The largest scope a grant or an instance invite carries ([`Grant::Account`]), as compact
/// JSON.
pub const MAX_GRANT_SCOPE_BYTES: usize = 256;
/// Team grants one instance holds at most.
pub const MAX_TEAM_GRANTS: usize = 16;
/// Account grants one instance holds at most (members and guests together).
pub const MAX_ACCOUNT_GRANTS: usize = 1_000;
/// Grants one instance holds at most: its teams, its names, the organization and anyone.
pub const MAX_GRANTS: usize = MAX_TEAM_GRANTS + MAX_ACCOUNT_GRANTS + 2;

/// Whether `scope` is within [`MAX_GRANT_SCOPE_BYTES`].
#[must_use]
pub fn scope_fits(scope: &serde_json::Value) -> bool {
    serde_json::to_vec(scope).is_ok_and(|b| b.len() <= MAX_GRANT_SCOPE_BYTES)
}

impl Access {
    /// Access through these grants.
    #[must_use]
    pub const fn new(grants: Vec<Grant>) -> Self {
        Self(grants)
    }

    /// The whole organization: what an instance gets when its creator names no grants.
    #[must_use]
    pub fn org() -> Self {
        Self(vec![Grant::Org])
    }

    /// One team: what giving an instance an app team does.
    #[must_use]
    pub fn team(team: TeamId) -> Self {
        Self(vec![Grant::Team { team }])
    }

    /// Its grants.
    #[must_use]
    pub fn grants(&self) -> &[Grant] {
        &self.0
    }

    /// The teams it admits.
    #[must_use]
    pub fn teams(&self) -> Vec<&TeamId> {
        self.0
            .iter()
            .filter_map(|g| match g {
                Grant::Team { team } => Some(team),
                _ => None,
            })
            .collect()
    }

    /// The one team it admits, when it admits exactly one: the team whose leads manage it.
    #[must_use]
    pub fn the_team(&self) -> Option<&TeamId> {
        match self.teams().as_slice() {
            [team] => Some(team),
            _ => None,
        }
    }

    /// Whether it admits the whole organization.
    #[must_use]
    pub fn org_wide(&self) -> bool {
        self.0.contains(&Grant::Org)
    }

    /// Whether it admits any signed-in account.
    #[must_use]
    pub fn anyone(&self) -> bool {
        self.0.contains(&Grant::Anyone)
    }

    /// Whether it names `account`.
    #[must_use]
    pub fn names(&self, account: &str) -> bool {
        self.0
            .iter()
            .any(|g| matches!(g, Grant::Account { account: a, .. } if a == account))
    }

    /// How many accounts it names.
    #[must_use]
    pub fn named_count(&self) -> usize {
        self.0
            .iter()
            .filter(|g| matches!(g, Grant::Account { .. }))
            .count()
    }

    /// Whether it admits named people only, with no member grant (as a private list or a
    /// direct message does).
    #[must_use]
    pub fn named_only(&self) -> bool {
        self.named_count() > 0 && !self.org_wide() && self.teams().is_empty()
    }

    /// The scope of `account`'s grant, if it names them with one.
    #[must_use]
    pub fn scope_of(&self, account: &str) -> Option<&serde_json::Value> {
        self.0.iter().find_map(|g| match g {
            Grant::Account {
                account: a,
                scope: Some(scope),
            } if a == account => Some(scope),
            _ => None,
        })
    }

    /// This access with `account` named (with `scope`). Already named: as it is.
    #[must_use]
    pub fn naming(&self, account: &str, scope: Option<serde_json::Value>) -> Self {
        if self.names(account) {
            return self.clone();
        }
        let mut grants = self.0.clone();
        grants.push(Grant::Account {
            account: account.to_owned(),
            scope,
        });
        Self(grants)
    }

    /// This access without its member grants (the organization, teams), `head` first: what
    /// giving an instance a team, or the organization in place of its teams, does.
    fn members_replaced_by(&self, head: Grant) -> Self {
        let mut grants = vec![head];
        grants.extend(
            self.0
                .iter()
                .filter(|g| !matches!(g, Grant::Org | Grant::Team { .. }))
                .cloned(),
        );
        Self(grants)
    }

    /// This access with its member grants (the organization, teams) replaced by `team`:
    /// what giving an instance an app team does. Its people and `anyone` stay.
    #[must_use]
    pub fn onto_team(&self, team: TeamId) -> Self {
        self.members_replaced_by(Grant::Team { team })
    }

    /// This access with its team grants replaced by the organization: what an instance on a
    /// team keeps where that team means nothing (another organization). Without team grants:
    /// as it is.
    #[must_use]
    pub fn teams_to_org(&self) -> Self {
        if self.teams().is_empty() {
            return self.clone();
        }
        self.members_replaced_by(Grant::Org)
    }

    /// This access without the team grants in `gone` (teams deleted).
    #[must_use]
    pub fn without_teams(&self, gone: &[TeamId]) -> Self {
        Self(
            self.0
                .iter()
                .filter(|g| !matches!(g, Grant::Team { team } if gone.contains(team)))
                .cloned()
                .collect(),
        )
    }

    /// Its public links ([`Grant::Link`]).
    #[must_use]
    pub fn links(&self) -> Vec<&LinkGrant> {
        self.0
            .iter()
            .filter_map(|g| match g {
                Grant::Link(l) => Some(l),
                _ => None,
            })
            .collect()
    }

    /// Its public link under `key`.
    #[must_use]
    pub fn link(&self, key: &str) -> Option<&LinkGrant> {
        self.links().into_iter().find(|l| l.link == key)
    }

    /// This access with `link` added.
    #[must_use]
    pub fn with_link(&self, link: LinkGrant) -> Self {
        let mut grants = self.0.clone();
        grants.push(Grant::Link(link));
        Self(grants)
    }

    /// This access without the public link under `key`.
    #[must_use]
    pub fn without_link(&self, key: &str) -> Self {
        Self(
            self.0
                .iter()
                .filter(|g| !matches!(g, Grant::Link(l) if l.link == key))
                .cloned()
                .collect(),
        )
    }

    /// `to`, the access a `SetAccess` names, with this access's public links kept. `None` when
    /// `to` names a link this access does not hold (links are made only with their own
    /// consent).
    #[must_use]
    pub fn keeping_links(&self, to: Self) -> Option<Self> {
        let kept = self.links();
        if to.links().iter().any(|l| !kept.contains(l)) {
            return None;
        }
        let mut grants: Vec<Grant> =
            to.0.into_iter()
                .filter(|g| !matches!(g, Grant::Link(_)))
                .collect();
        grants.extend(kept.into_iter().cloned().map(Grant::Link));
        Some(Self(grants))
    }

    /// This access without its public links: what anyone but its managers sees.
    #[must_use]
    pub fn without_links(&self) -> Self {
        Self(
            self.0
                .iter()
                .filter(|g| !matches!(g, Grant::Link(_)))
                .cloned()
                .collect(),
        )
    }

    /// Only `account`'s own grant, as a guest sees the instance's access: none of the
    /// organization's people.
    #[must_use]
    pub fn only(&self, account: &str) -> Self {
        Self(
            self.0
                .iter()
                .filter(|g| matches!(g, Grant::Account { account: a, .. } if a == account))
                .cloned()
                .collect(),
        )
    }
}

/// What a registered node is for: a replica holds apps and the cluster key, a
/// job node only runs compute for them and never holds either.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeRole {
    #[default]
    Replica,
    Job,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRecord {
    pub id: u64,
    pub addr: String,
    #[serde(default)]
    pub zone: String,
    #[serde(default)]
    pub role: NodeRole,
    /// The compute pool a job node serves; empty is [`crate::system::DEFAULT_POOL`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pool: String,
}

/// Which components a release, and the app made from it, has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Components {
    /// An app component, with a compute component when the release ships one.
    #[default]
    App,
    /// A compute component only: nothing to call, view, initialise or tick.
    ComputeOnly,
}

impl Components {
    #[must_use]
    pub fn is_app(&self) -> bool {
        *self == Self::App
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppRecord {
    pub app_id: AppId,
    pub name: String,
    pub group: u64,
    pub bundle: String,
    pub owner: AccountId,
    /// The organization the app belongs to. Every app has one: an app made
    /// without naming one is in its owner's personal organization
    /// ([`Org::personal`]). Apps outside any organization were removed in a
    /// hard cutover (1.0.0-rc.20).
    pub org: OrgId,
    /// Who reaches it ([`Access`]), as `grants`.
    #[serde(default, rename = "grants")]
    pub access: Access,
    /// Methods that needed a fresh passkey assertion (`#[app::confirm]`), as an
    /// SDK 0.12 app's record names them; read by nothing now (a component's come
    /// from its ABI), kept so a record writes as it read.
    #[serde(default)]
    pub confirm_methods: Vec<String>,
    /// The executor its bundle runs on, decided once from the bundle's WASM when
    /// the app is created: a component. Absent (a record written before components
    /// existed): wasmer, an SDK 0.12 app this node no longer runs.
    #[serde(default, skip_serializing_if = "Executor::is_wasmer")]
    pub executor: Executor,
    pub replicas: Vec<u64>,
    pub created_at: u64,
    /// The arguments `init` ran with (JSON; `null` for none).
    #[serde(default)]
    pub init_args: serde_json::Value,
    /// The compute component of the release `bundle` came from, set with `bundle` at create
    /// and upgrade; `None` when that release ships none (or `bundle` is a raw upload).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compute_bundle: Option<String>,
    /// The components it runs; `bundle` and `compute_bundle` both name the compute
    /// component when it has no app component.
    #[serde(default, skip_serializing_if = "Components::is_app")]
    pub components: Components,
    /// The app instance's metadata: a title, an icon.
    /// Set by its owner or an admin of its organization.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// The last upgrade tried on this app, done or failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_upgrade: Option<UpgradeRecord>,
    /// The deployment it was created from (`system::Deployment`), at a
    /// release that deployment offered then. Absent (a record from before,
    /// or an instance made from a release directly): none, and it is left
    /// out, so such a record writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployment: Option<String>,
    /// The package release this instance runs, set at create (the release
    /// named, or the one its deployment offered) and at every upgrade, one
    /// to a release with the same WASM included: releases may share a WASM
    /// and differ in their frontend, which works with its own release only,
    /// so `bundle` alone does not say which one it runs. Absent (a record
    /// from before, or a raw upload): the release is inferred from `bundle`
    /// (`report::release_of_bundle`), and it is left out, so such a record
    /// writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub release: Option<crate::system::PackageRef>,
    /// A restore from a backup under way (`SystemCmd::RestoreApp`): the app's
    /// new group is being seeded, and calls wait for it. Left out when `None`,
    /// so a record writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<PendingRestore>,
    /// The apps this instance's release creates with it (`uses` with `mode: create` in its
    /// signed manifest), by the name its code knows each by: made in the same
    /// organization, by the same owner, just before it. Set once, at create. Left out when
    /// empty, so a record writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub links: BTreeMap<String, AppId>,
    /// The apps its owner attached for the release's `attach` references (`uses` with
    /// `mode: attach`), by name: apps of the same organization that live on their own
    /// (`SystemCmd::LinkApp`). Never a unit member, so a unit's upgrade and range leave them
    /// alone. Left out when empty, so a record writes as it did.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub attached: BTreeMap<String, AppId>,
    /// The restore point of its last upgrade, which its owner or an admin may undo
    /// for [`UNDO_WINDOW_SECS`] ([`RestorePoint`]). Left out when `None`, so a record
    /// writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore_point: Option<RestorePoint>,
}

/// The backup a restore takes an app's state from (`crate::backup`): the
/// snapshot file's name, its SHA-256 and the app log's index it was taken at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreSource {
    pub name: String,
    pub sha256: String,
    pub index: u64,
}

/// A restore under way: `seed` (the first of the app's replicas) holds the
/// backup and seeds the app's new group from it, then adds the other replicas.
/// Without a seed, the app waits for one: the cluster was founded again from
/// its system backup (`node::restore`), and no node hosts the app until the
/// operator restores it from its own backup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingRestore {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<RestoreSource>,
}

impl PendingRestore {
    /// What an app of a cluster founded again from its system backup waits in.
    pub const AWAITING_BACKUP: Self = Self {
        seed: None,
        source: None,
    };

    /// Whether the app waits for a backup: no node hosts it meanwhile.
    #[must_use]
    pub const fn awaits_backup(&self) -> bool {
        self.seed.is_none()
    }
}

impl AppRecord {
    /// `422 NO_APP_COMPONENT` unless the app has an app component to call, view, initialise or tick.
    pub fn require_app_component(&self) -> Result<(), crate::error::ApiError> {
        if self.components.is_app() {
            Ok(())
        } else {
            Err(crate::error::ApiError::no_app_component())
        }
    }
}

/// Which executor runs an app's code.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Executor {
    /// An SDK 0.12 core module, which ran on wasmer. This node has no wasmer: such
    /// an app is listed and can be deleted, and every call, view and upgrade of it
    /// is `410 UNSUPPORTED_ENGINE` ([`crate::error::ApiError::unsupported_engine`]).
    /// The default, as a record from before components names it by omission.
    #[default]
    Wasmer,
    /// A `calimero:app` component (the v2 SDK), on wasmtime through the host
    /// core (`calimero-app-host`).
    Component,
}

impl Executor {
    /// The executor for `wasm`: a component (its header's layer field says so,
    /// `wasmparser::Parser::is_component`) runs on wasmtime; anything else is a
    /// core module, which nothing here runs (`400 UNSUPPORTED_BUNDLE`).
    #[must_use]
    pub fn of(wasm: &[u8]) -> Self {
        if wasmparser::Parser::is_component(wasm) {
            Self::Component
        } else {
            Self::Wasmer
        }
    }

    #[must_use]
    pub fn is_wasmer(&self) -> bool {
        *self == Self::Wasmer
    }
}

/// One upgrade of one app, as the directory remembers it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeRecord {
    pub from: String,
    pub to: String,
    /// Unix seconds.
    pub at: u64,
    /// The account that upgraded it, or `operator`.
    pub by: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "serde_json::Value::is_null")]
    pub error: serde_json::Value,
    /// The organization-wide run it was part of, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The upgrade key (its id) that made it, in place of a passkey.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    /// The release the app ran before (`AppRecord::release`), and the one it
    /// was moved to: `from` and `to` alone do not tell apart releases that
    /// share a WASM. Absent on a record from before, or for a raw upload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_release: Option<crate::system::PackageRef>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_release: Option<crate::system::PackageRef>,
    /// The class the update applied as (`UpgradeApp::class`), judged against the log where
    /// it applied. Absent on a record from before, for a raw upload or a failed upgrade.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<calimero_bundle::update::Class>,
    /// Why it was red; empty when green or unclassified.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<calimero_bundle::update::Reason>,
    /// The owner or admin whose first approval ([`PendingApproval`]) let this red update
    /// through, where the app's policy asks for a second approver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approved_by: Option<AccountId>,
    /// It applied on its own under the app's update policy (`UpgradeApp::auto`), `by`
    /// naming no person ([`crate::system::AUTO_UPDATE`]). Left out when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto: bool,
    /// It undid the upgrade before it (`SystemCmd::UndoUpgrade`): `from_release` is the
    /// release undone, `to_release` the one restored. Left out when `false`.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub undo: bool,
}

/// How long after an upgrade its owner or an admin may undo it
/// (`SystemCmd::UndoUpgrade`), judged on the logged times of the upgrade and the undo: a
/// week. Past it, the restore point is spent.
pub const UNDO_WINDOW_SECS: u64 = 7 * 24 * 3600;

/// A restore point's file, kept by the replica that took it in its
/// `<data>/restore-points/<app>/` (`crate::node::restore_points`): the app group's sealed
/// snapshot as `crate::backup` writes one, taken just before the upgrade it undoes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestorePointFile {
    /// The node that holds it, and seeds the app's group from it on an undo.
    pub node: u64,
    /// The app's group the snapshot was taken of.
    pub group: u64,
    /// The file's name, its SHA-256 and the group's log index it holds.
    pub source: RestoreSource,
}

/// The restore point an upgrade took before it ran (`UpgradeApp::restore_point`): the
/// app group's snapshot (`file`), or none for a move that runs nothing on the group (a
/// release with the app's own bundle, a compute-only app), which an undo reverts in the
/// directory alone.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TakenRestorePoint {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<RestorePointFile>,
}

/// The one undo an app keeps (`AppRecord::restore_point`): its last upgrade, which
/// `SystemCmd::UndoUpgrade` reverts within [`UNDO_WINDOW_SECS`] to the release and the
/// state it ran before (the one move to an older release). Replaced by every upgrade,
/// dropped by an undo once its group holds the restored state, and by an upgrade that
/// took none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestorePoint {
    /// What the app ran before the upgrade, which an undo restores.
    pub from_bundle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_compute: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_release: Option<crate::system::PackageRef>,
    /// What the upgrade moved it to: an undo is refused once the app runs anything else.
    pub to_bundle: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_release: Option<crate::system::PackageRef>,
    /// The upgrade's logged time: the undo window starts here.
    pub at: u64,
    /// Who upgraded it (an account, or [`crate::system::AUTO_UPDATE`]).
    pub by: String,
    /// The run it was part of: a unit's members undo with their instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    /// The app group's snapshot; `None` when the upgrade ran nothing on the group.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<RestorePointFile>,
    /// When an undo of it was logged: the app is being restored from `file` (a retry may
    /// seed it again until the restore finishes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub undone_at: Option<u64>,
}

impl RestorePoint {
    /// Whether an undo logged at `at` is within [`UNDO_WINDOW_SECS`] of the upgrade.
    #[must_use]
    pub const fn open_at(&self, at: u64) -> bool {
        at <= self.at.saturating_add(UNDO_WINDOW_SECS)
    }
}

/// An upgrade of many apps at once, with cascade: every
/// app of the organization (or of a team, and with `cascade` the teams below
/// it) that runs `package`, to `version`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpgradeRun {
    pub run_id: String,
    pub package: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<TeamId>,
    #[serde(default)]
    pub cascade: bool,
    /// The account that started it, or `operator`.
    pub by: String,
    pub started_at: u64,
    pub apps: Vec<AppId>,
    pub done: Vec<AppId>,
    /// App -> why its upgrade failed.
    pub failed: std::collections::BTreeMap<AppId, serde_json::Value>,
    pub finished: bool,
    /// A unit's run (`__upgrade_unit`): each member's own target, a release of its own
    /// package (`package` and `version` above are the instance's). An app without one is
    /// moved to `package`@`version`, as every other run does. Left out when empty, so a run
    /// writes as it did.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub targets: std::collections::BTreeMap<AppId, RunTarget>,
}

/// A unit member's release in a unit's run (`UpgradeRun::targets`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunTarget {
    pub package: String,
    pub version: String,
}

impl UpgradeRun {
    /// The package and version run `self` moves `app` to: its own target in a unit's run,
    /// else the run's.
    #[must_use]
    pub fn target_of(&self, app: &str) -> (&str, &str) {
        self.targets
            .get(app)
            .map_or((self.package.as_str(), self.version.as_str()), |t| {
                (t.package.as_str(), t.version.as_str())
            })
    }
}

/// How many upgrade runs an organization remembers.
pub const UPGRADE_RUNS_KEPT: usize = 20;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Account {
    pub account_id: AccountId,
    pub name: String,
    pub created_at: u64,
    pub passkeys: Vec<Credential>,
    /// A name for each of the account's passkeys (its devices), by credential
    /// id: a device label.
    #[serde(default)]
    pub passkey_labels: std::collections::BTreeMap<String, String>,
    /// The account's own names for apps (aliases): alias -> app id.
    #[serde(default)]
    pub aliases: std::collections::BTreeMap<String, AppId>,
    /// When each passkey was added and last signed in, by credential id,
    /// shown to the account holder alone (`/auth/me`). Written at apply from
    /// the times the log entries carry; a passkey with no entry, or a time
    /// absent, was added or signed in before this was kept and reads as
    /// unknown. Left out when empty, so accounts from before, and the
    /// account a sign-up's passkey signs (`system::account_args`), read and
    /// write as they did.
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub passkey_times: std::collections::BTreeMap<String, PasskeyTimes>,
}

/// How many activity entries (`system::SystemState::activity`) an account keeps: the oldest goes
/// first.
pub const ACTIVITY_KEPT: usize = 100;

/// The longest method or subject an [`Activity::Approval`] keeps, in
/// characters (they are names the node makes: ids and `__` methods).
pub const ACTIVITY_TEXT_CHARS: usize = 160;

/// One thing a passkey did, at `at` (Unix seconds, as the node that took
/// the ceremony read its clock).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ActivityEntry {
    pub at: u64,
    /// The credential id of the passkey that did it.
    pub passkey: String,
    #[serde(flatten)]
    pub what: Activity,
}

/// What an [`ActivityEntry`] records.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Activity {
    /// A sign-in (a sign-up's, a recovery's finish): the package whose
    /// frontend opened the session and the registered origin, if bound.
    #[serde(rename_all = "camelCase")]
    SignIn {
        #[serde(default)]
        app: Option<String>,
        #[serde(default)]
        origin: Option<String>,
    },
    /// A change the passkey approved (a step-up): the method and subject
    /// its consent states (`proof::Statement`), never its arguments.
    #[serde(rename_all = "camelCase")]
    Approval { method: String, subject: String },
}

/// Records `entry` in an account's activity `log`, dropping the oldest past
/// [`ACTIVITY_KEPT`]. A sign-in that repeats the newest entry's passkey, app
/// and origin only moves its time, so signing in again to one app keeps the
/// rest. An entry with no time (0) records nothing.
pub fn record_activity(log: &mut std::collections::VecDeque<ActivityEntry>, entry: ActivityEntry) {
    if entry.at == 0 {
        return;
    }
    if let Some(last) = log.back_mut() {
        if matches!(last.what, Activity::SignIn { .. })
            && last.what == entry.what
            && last.passkey == entry.passkey
        {
            last.at = last.at.max(entry.at);
            return;
        }
    }
    log.push_back(entry);
    while log.len() > ACTIVITY_KEPT {
        let _oldest = log.pop_front();
    }
}

/// When one passkey was added and last signed in (Unix seconds, as the node
/// that took the ceremony read its clock). `None`: not known, because it
/// happened before the node kept it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PasskeyTimes {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<u64>,
    /// The last sign-in with it (a sign-up, a sign-in or a recovery's
    /// finish): the log entry each already writes. Refreshes do not move it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sign_in_at: Option<u64>,
}

impl PasskeyTimes {
    /// Records a sign-in at `at` (0: unknown, nothing changes); the latest
    /// wins, whatever order sign-ins committed in.
    pub fn signed_in(&mut self, at: u64) {
        if at > 0 {
            self.last_sign_in_at = Some(self.last_sign_in_at.map_or(at, |t| t.max(at)));
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ApiKeyRecord {
    pub key_id: String,
    pub account: AccountId,
    pub label: String,
    /// `None` = every app the account can reach.
    #[serde(default)]
    pub apps: Option<Vec<AppId>>,
    /// `None` = every method.
    #[serde(default)]
    pub methods: Option<Vec<String>>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub allow_confirm: bool,
    pub expires_at: u64,
    /// A key that upgrades one app and does nothing else: no passkey on each
    /// upgrade, only to signed releases of one package by its pinned signer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrades: Option<UpgradeScope>,
}

/// What an upgrade key ([`ApiKeyRecord::upgrades`]) may do: upgrade `app`
/// to a release of `package` signed by `signer`, proving each upgrade with
/// the Ed25519 key its secret stands for (`proof::api_key_public`), so a
/// node that saw only the log cannot forge one.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradeScope {
    pub app: AppId,
    pub package: String,
    /// The package's pinned signer (`did:key`) when the key was made.
    pub signer: String,
    /// The key's public half, hex.
    pub proof_key: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RefreshRecord {
    pub account: AccountId,
    pub session: String,
    pub expires_at: u64,
    /// The passkey the session was opened with.
    #[serde(default)]
    pub passkey: Option<String>,
    /// The package whose frontend opened the session, which binds it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
    /// The registered origin that opened it (`SessionClaims::origin`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
    /// The token's public key (`proof::refresh_key`, hex), which signs its
    /// exchange (`UseRefresh`). Empty in records from before: those are not
    /// exchanged, and their holders sign in again.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// When the session was opened (Unix seconds), carried by every exchange
    /// of its token (`UseRefresh`). 0: opened before this was kept, unknown.
    /// Left out when 0, so the records from before, and the record a
    /// sign-up's passkey signs (`system::account_args`), read and write as
    /// they did.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub opened_at: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrgRole {
    Guest,
    Member,
    Admin,
    Owner,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamJoin {
    /// Open: everyone who reaches the parent team (or,
    /// at the top, the organization) is a member too, unless an admin denied
    /// them `caps::JOIN_OPEN_TEAMS`.
    Everyone,
    /// Restricted: explicit members only.
    Invite,
}

/// What a member may do beyond their role: v1's `MemberCapabilities` bits, at
/// the same values. A team's lead and an organization admin hold them all.
pub mod caps {
    pub const CREATE_APP: u32 = 1 << 0;
    pub const INVITE_MEMBERS: u32 = 1 << 1;
    /// Inherited into open teams; denying it keeps a member out of them.
    pub const JOIN_OPEN_TEAMS: u32 = 1 << 2;
    pub const MANAGE_MEMBERS: u32 = 1 << 3;
    pub const MANAGE_APPLICATION: u32 = 1 << 4;
    /// Create a top-level team without being an admin.
    pub const CREATE_TEAM: u32 = 1 << 5;
    /// Delete a team (and what it holds) without being an admin.
    pub const DELETE_TEAM: u32 = 1 << 6;
    pub const MANAGE_VISIBILITY: u32 = 1 << 7;
    pub const MANAGE_METADATA: u32 = 1 << 8;
    /// Approve (or withdraw) the mero-tee releases that may hold the
    /// organization's apps, held organization-wide only. Bit 10, not 9: v1's
    /// `MemberCapabilities` spends 9 on `CAN_AUTHOR_ON_BEHALF`, so a client
    /// that sends v1's `ALL` does not hand out release approvals with it.
    pub const APPROVE_RELEASES: u32 = 1 << 10;
    /// Create a restricted team below a team one writes (a private channel
    /// under an app's team) without leading it; the creator leads the new
    /// team. On by default in an app team (its default capabilities).
    pub const CREATE_CHILD_TEAM: u32 = 1 << 11;
    /// Every capability there is (bit 9 is v1's, never handed out).
    pub const DEFINED: u32 = ((1 << 9) - 1) | APPROVE_RELEASES | CREATE_CHILD_TEAM;
    /// What a member holds unless an admin says otherwise.
    pub const DEFAULT: u32 = JOIN_OPEN_TEAMS;
}

/// A member's standing in one team (a direct row).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TeamRole {
    /// Manages the team and the teams below it.
    Lead,
    Member,
    /// Reads, never writes, the team's apps.
    ReadOnly,
}

fn default_caps() -> u32 {
    caps::DEFAULT
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub team_id: TeamId,
    pub name: String,
    #[serde(default)]
    pub parent: Option<TeamId>,
    pub join: TeamJoin,
    /// account -> is lead
    #[serde(default)]
    pub members: std::collections::BTreeMap<AccountId, bool>,
    /// Members whose row is read-only: they write none
    /// of the team's apps. Only a direct row counts.
    #[serde(default)]
    pub read_only: std::collections::BTreeSet<AccountId>,
    /// Capabilities per direct row ([`caps`]).
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<AccountId, u32>,
    /// What the app stores about the team (a folder's name and colour).
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Members a manager removed: they do not come back by inheritance, only
    /// when someone adds them again.
    #[serde(default)]
    pub removed: std::collections::BTreeSet<AccountId>,
    /// Capabilities ([`caps`]) everyone in the team holds there on top of
    /// their organization-wide ones, unless their row sets its own: "members
    /// may invite others to this app". Absent in teams from before, and left
    /// out when unset, so such a team reads and writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_capabilities: Option<u32>,
}

impl Team {
    /// The package this team is the app team of: `metadata.app.package` on a
    /// top-level team ([`Org::app_team`]). One per package per organization.
    #[must_use]
    pub fn app_package(&self) -> Option<&str> {
        if self.parent.is_some() {
            return None;
        }
        self.metadata
            .get("app")
            .and_then(|a| a.get("package"))
            .and_then(serde_json::Value::as_str)
    }
}

/// How an organization holds its apps, derived from its app teams
/// ([`Team::app_package`]), never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OrgMode {
    /// No app team: its apps use organization access, as every organization
    /// made before app teams does. Behaves as standalone.
    Legacy,
    /// Every app team is open: everyone in the organization has its apps.
    Standalone,
    /// Some app team is closed: each app lists who is in it, and members see
    /// only the people of the apps they are in.
    Shared,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Org {
    pub org_id: OrgId,
    pub name: String,
    pub members: std::collections::BTreeMap<AccountId, OrgRole>,
    pub teams: std::collections::BTreeMap<TeamId, Team>,
    /// Capabilities a member holds unless set for them.
    #[serde(default = "default_caps")]
    pub default_capabilities: u32,
    #[serde(default)]
    pub capabilities: std::collections::BTreeMap<AccountId, u32>,
    /// mero-tee releases the owner approved to hold this organization's apps
    /// (the release the org was created on, then each one the owner approves).
    /// A node on any other release is sent none of their data.
    #[serde(default)]
    pub approved_releases: std::collections::BTreeSet<String>,
    /// Outbound URL prefixes the owner approved, by package signer (`did:key`):
    /// an app's run reaches only a prefix its manifest declares and this holds.
    #[serde(default)]
    #[serde(skip_serializing)]
    pub outbound_approvals: std::collections::BTreeMap<String, OutboundApproval>,
    /// The organization's own metadata, for its apps
    /// and frontends to read: a name's display form, a logo, settings.
    #[serde(default)]
    pub metadata: serde_json::Value,
    /// Per-member metadata: a display name, a role
    /// title. Set by the member, or by an admin.
    #[serde(default)]
    pub member_metadata: std::collections::BTreeMap<AccountId, serde_json::Value>,
    /// Organization-wide upgrade runs, the newest [`UPGRADE_RUNS_KEPT`].
    #[serde(default)]
    pub upgrade_runs: std::collections::BTreeMap<String, UpgradeRun>,
    /// The organization's recovery key and any recovery under way
    /// (`docs/specs/org-recovery.md`); absent for an organization that never
    /// set one, as every organization written before this field was.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<OrgRecovery>,
    /// Members who get every app added to the organization from now on: each
    /// app team made after (`ScopeApps`, `MoveApp`, a team marked as one)
    /// gives them a plain row, whoever else it names. Set by an admin's
    /// invite ([`Invite::all_apps`]) or later by an admin
    /// (`SystemCmd::SetAllApps`); gone with their membership. Left out when
    /// empty, so an organization from before writes as it did.
    #[serde(default, skip_serializing_if = "std::collections::BTreeSet::is_empty")]
    pub all_apps: std::collections::BTreeSet<AccountId>,
    /// Made for one account to hold its instances (`CreateOrg { personal }`):
    /// an instance created without naming an organization goes to its
    /// owner's personal organization, made the first time. An account owns
    /// at most one, and it does not count against `maxOrgsPerAccount`. It is
    /// an organization like any other otherwise (its owner may invite
    /// people). Left out when false, so an organization from before writes
    /// as it did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub personal: bool,
    /// Whether its instances may take guests ([`crate::system::SystemState::guest_reaches`]):
    /// the organization's switch, beside each release's (`PackageVersion::guests`); the
    /// stricter wins. Set by its owner or an admin (`SystemCmd::SetOrgSettings`). Left out
    /// when `true` (the default), so an organization from before writes as it did.
    #[serde(default = "allowed", skip_serializing_if = "is_allowed")]
    pub allow_guests: bool,
    /// Whether its instances' public links admit visitors
    /// ([`crate::system::SystemState::public_link_admits`]); off, creating one is refused
    /// and existing ones admit nobody. Set like [`Self::allow_guests`]; left out when
    /// `true` (the default).
    #[serde(default = "allowed", skip_serializing_if = "is_allowed")]
    pub allow_public_links: bool,
    /// The update policy of every app of it that sets none of its own
    /// (`SystemCmd::SetUpgradePolicy` without an app); absent, [`UpgradePolicy::default`]:
    /// nothing updates on its own and one owner's or admin's approval applies an update.
    /// Left out when absent, so an organization from before writes as it did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upgrade_policy: Option<UpgradePolicy>,
    /// An app's own update policy, by app, over [`Self::upgrade_policy`]
    /// (`SystemCmd::SetUpgradePolicy` naming the app). Kept by the organization, so an app
    /// moved out leaves it behind (`MoveApp` drops it): the new organization's owner and
    /// admins decide. Left out when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub app_upgrade_policies: BTreeMap<AppId, UpgradePolicy>,
    /// First approvals of red updates whose policy asks for a second approver
    /// (`SystemCmd::ApproveUpgrade`), by the app approved: one each, replaced by a newer
    /// one, and dead once its apps run their targets or it is older than
    /// [`MAX_APPROVAL_AGE_SECS`]. Left out when empty.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub pending_upgrades: BTreeMap<AppId, PendingApproval>,
}

/// The shortest hold an update policy names ([`UpgradePolicy::hold_hours`]): Calimero's
/// minimum, so a bad release can be pulled before it applies anywhere on its own.
pub const MIN_HOLD_HOURS: u32 = 24;
/// The longest hold an update policy names: a week.
pub const MAX_HOLD_HOURS: u32 = 168;
/// How long a first approval ([`PendingApproval`]) waits for its second, judged at the
/// time the upgrade it would let through carries: a week.
pub const MAX_APPROVAL_AGE_SECS: u64 = 7 * 24 * 3600;
/// The most reasons an update's class lists ([`UpgradeClass::reasons`]).
pub const MAX_CLASS_REASONS: usize = 64;

/// How an organization's owner and admins let an app's updates apply
/// (`SystemCmd::SetUpgradePolicy`), for one app or every app of the organization. Every
/// field may be left out (its default); an unknown one is refused.
///
/// `second_approver` asks for a second owner's or admin's approval of a red update;
/// `auto_apply` and `hold_hours` let a green one apply on its own after its hold, with a
/// restore point (`node::auto_updates`, `SystemState::auto_update_gate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradePolicy {
    /// Which updates apply on their own after the hold ([`AutoApply`]): none by default.
    #[serde(default)]
    pub auto_apply: AutoApply,
    /// How long a green update waits before it applies on its own, in hours:
    /// [`MIN_HOLD_HOURS`] to [`MAX_HOLD_HOURS`], 24 by default.
    #[serde(default = "default_hold_hours")]
    pub hold_hours: u32,
    /// A red update (or one no class was recorded for) needs two distinct owners or admins:
    /// the first logs a [`PendingApproval`], the second applies it. Off by default.
    #[serde(default)]
    pub second_approver: bool,
}

/// Which updates an update policy lets apply on their own, after its hold
/// ([`UpgradePolicy::auto_apply`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AutoApply {
    /// None: every update waits for an owner's or admin's approval. The default: nothing
    /// updates by default.
    #[default]
    Never,
    /// A green update (the same signer and state version, no new destination, no new way to
    /// act as the person) of an app whose unit has no destinations before or after it (no
    /// `outbound` prefix or secret host, members counted: no data leaves Calimero).
    NoDestinations,
}

const fn default_hold_hours() -> u32 {
    MIN_HOLD_HOURS
}

impl Default for UpgradePolicy {
    fn default() -> Self {
        Self {
            auto_apply: AutoApply::Never,
            hold_hours: default_hold_hours(),
            second_approver: false,
        }
    }
}

impl UpgradePolicy {
    /// `Err` with the apply error when `hold_hours` is outside
    /// [`MIN_HOLD_HOURS`]..=[`MAX_HOLD_HOURS`].
    pub fn check(&self) -> Result<(), &'static str> {
        if (MIN_HOLD_HOURS..=MAX_HOLD_HOURS).contains(&self.hold_hours) {
            Ok(())
        } else {
            Err(crate::system::BAD_UPGRADE_POLICY)
        }
    }
}

/// What the classifier (`calimero_bundle::update::classify`) said of an update, as the
/// log keeps it: green, or red with every reason.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UpgradeClass {
    pub class: calimero_bundle::update::Class,
    /// Every rule that makes it red; empty when green. At most [`MAX_CLASS_REASONS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reasons: Vec<calimero_bundle::update::Reason>,
}

impl UpgradeClass {
    /// Whether it is green.
    #[must_use]
    pub fn is_green(&self) -> bool {
        self.class == calimero_bundle::update::Class::Green
    }
}

/// The first of the two approvals a red update needs where its policy asks for a second
/// approver (`SystemCmd::ApproveUpgrade`): the app to `package`@`version`, and for a unit
/// each member to its own target.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PendingApproval {
    pub package: String,
    pub version: String,
    /// A unit's upgrade: each member it moves, by app, to its own release. Empty for the
    /// app alone.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub targets: BTreeMap<AppId, RunTarget>,
    /// The owner or admin who approved first.
    pub by: AccountId,
    /// When (Unix seconds, the approving node's clock, logged with it).
    pub at: u64,
    /// The class the approver was shown.
    pub class: UpgradeClass,
}

impl PendingApproval {
    /// Whether it lets `app` move to `package`@`version`: the approved app itself, or a
    /// member of its unit to the member's own target.
    #[must_use]
    pub fn covers(&self, approved: &str, app: &str, package: &str, version: &str) -> bool {
        if approved == app {
            return self.package == package && self.version == version;
        }
        self.targets
            .get(app)
            .is_some_and(|t| t.package == package && t.version == version)
    }

    /// Whether it may still let an upgrade logged at `at` through.
    #[must_use]
    pub const fn fresh_at(&self, at: u64) -> bool {
        at <= self.at.saturating_add(MAX_APPROVAL_AGE_SECS)
    }
}

/// [`Org::allow_guests`]' default.
const fn allowed() -> bool {
    true
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_allowed(b: &bool) -> bool {
    *b
}

/// A public link a `CreatePublicLink` makes ([`crate::system::SystemCmd::CreatePublicLink`]):
/// its instance and terms. Applied, it is a [`LinkGrant`] in the instance's grant list:
/// whoever holds its token calls the instance's `#[app::public]` methods as an anonymous
/// visitor (`Caller::Visitor`), whatever its other grants.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PublicLink {
    pub app: AppId,
    /// A name its managers know it by (at most `MAX_PUBLIC_LINK_LABEL_CHARS`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// When it stops admitting visitors (Unix seconds); none: until revoked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    /// Calls one visitor may make through it a minute, counted on each node
    /// (`api::VisitorLimits`).
    pub rate_per_minute: u32,
    /// Who made it: the command's actor, set where it applies.
    #[serde(default)]
    pub created_by: AccountId,
    /// When it was made (Unix seconds, the creating node's clock).
    #[serde(default)]
    pub created_at: u64,
}

/// An instance invite ([`crate::system::SystemCmd::CreateAppInvite`]): whoever redeems it
/// is named on the instance (a [`Grant::Account`]: a guest, or a named member when they are
/// a member of its organization), never added to its organization. Kept under the token's
/// Ed25519 public key, as an organization invite.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AppInvite {
    pub app: AppId,
    pub expires_at: u64,
    pub uses_left: u32,
    /// Who made it: the command's actor, set where it applies.
    #[serde(default)]
    pub created_by: AccountId,
    /// When it was made (Unix seconds, the creating node's clock).
    #[serde(default)]
    pub created_at: u64,
    /// The scope the grant it makes carries ([`Grant::Account`]'s `scope`): opaque to the
    /// platform, at most [`MAX_GRANT_SCOPE_BYTES`]. Left out when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<serde_json::Value>,
}

/// An organization's recovery key: whoever holds its seed can hand the
/// organization to an account, [`RECOVERY_DELAY_SECS`] after starting, unless
/// the owner cancels first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrgRecovery {
    /// The key's Ed25519 public half, base64url
    /// ([`calimero_identity::recovery`]).
    pub public_key: String,
    /// When the owner set it (the setting command's time, Unix seconds).
    pub set_at: u64,
    /// A recovery under way.
    #[serde(default)]
    pub pending: Option<PendingRecovery>,
}

/// A recovery started with the seed, not finished or cancelled yet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingRecovery {
    /// The account the organization goes to.
    pub account: AccountId,
    /// When it started (Unix seconds).
    pub started_at: u64,
    /// The earliest time it may finish.
    pub ready_at: u64,
}

/// How long a started recovery, an organization's or an account's, waits
/// before it may finish: the owner's time to notice and cancel (72 hours).
pub const RECOVERY_DELAY_SECS: u64 = 72 * 60 * 60;

/// An account recovery started with the account's recovery code, not
/// finished or cancelled yet (`docs/specs/account-recovery.md`): the passkey
/// it adds, [`RECOVERY_DELAY_SECS`] after the start (the same delay as an
/// organization's), unless one of the account's passkeys cancels first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PendingAccountRecovery {
    /// The new passkey, registered at the start.
    pub credential: Credential,
    /// When it started (the start command's time, Unix seconds).
    pub started_at: u64,
    /// The earliest time it may finish.
    pub ready_at: u64,
}

impl PendingAccountRecovery {
    /// What the start answers and `/auth/me` shows: never the credential's
    /// public key.
    #[must_use]
    pub fn json(&self, account: &str) -> serde_json::Value {
        serde_json::json!({
            "accountId": account,
            "credentialId": self.credential.id,
            "startedAt": self.started_at,
            "readyAt": self.ready_at,
        })
    }
}

/// The most bytes of JSON one metadata value holds (org, member, team, app).
pub const MAX_METADATA_BYTES: usize = 16 * 1024;

/// What an owner approved for one publisher's apps to call out to.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundApproval {
    /// The approved URL prefixes.
    pub prefixes: Vec<String>,
    /// Every prefix the owner was shown, approved or not: one declared later
    /// reads as awaiting approval, one left out as not approved.
    pub reviewed: Vec<String>,
}

impl Org {
    #[must_use]
    pub fn role(&self, account: &str) -> Option<OrgRole> {
        self.members.get(account).copied()
    }

    /// Whether `account` is a member of `team`: a direct row, or, for an
    /// open team, a member of its parent (the organization at the top) who
    /// holds [`caps::JOIN_OPEN_TEAMS`] there.
    #[must_use]
    pub fn in_team(&self, account: &str, team: &str) -> bool {
        self.in_team_at(account, team, 0)
    }

    fn in_team_at(&self, account: &str, team: &str, depth: usize) -> bool {
        if self.role(account).is_none() || depth > 32 {
            return false;
        }
        let Some(t) = self.teams.get(team) else {
            return false;
        };
        if t.members.contains_key(account) {
            return true;
        }
        if t.join != TeamJoin::Everyone || t.removed.contains(account) {
            return false;
        }
        match &t.parent {
            Some(p) => {
                // The team's default counts only for those in it, which the
                // recursion below requires anyway.
                let parent_caps = self.teams.get(p).map_or_else(
                    || self.org_caps(account),
                    |pt| self.row_caps(pt, account, true),
                );
                parent_caps & caps::JOIN_OPEN_TEAMS != 0 && self.in_team_at(account, p, depth + 1)
            }
            // Where apps have app teams a guest has only the apps of the
            // teams they are in: open teams are not theirs by inheritance.
            None => !self.app_only(account) && self.org_caps(account) & caps::JOIN_OPEN_TEAMS != 0,
        }
    }

    /// What `account` holds in `t`: their row's own capabilities, else their
    /// organization-wide ones with the team's default on top when they are in
    /// it (`in_team`).
    fn row_caps(&self, t: &Team, account: &str, in_team: bool) -> u32 {
        if let Some(c) = t.capabilities.get(account) {
            return *c;
        }
        let default = if in_team {
            t.default_capabilities.unwrap_or(0)
        } else {
            0
        };
        self.org_caps(account) | default
    }

    /// The capabilities `account`'s row in `team` holds, or would hold
    /// without one ([`Self::may`] below a lead).
    #[must_use]
    pub fn team_caps(&self, account: &str, team: &str) -> u32 {
        self.teams.get(team).map_or_else(
            || self.org_caps(account),
            |t| self.row_caps(t, account, self.in_team(account, team)),
        )
    }

    /// `account`'s direct row in `team`, if any.
    #[must_use]
    pub fn team_role(&self, account: &str, team: &str) -> Option<TeamRole> {
        let t = self.teams.get(team)?;
        let lead = *t.members.get(account)?;
        Some(if lead {
            TeamRole::Lead
        } else if t.read_only.contains(account) {
            TeamRole::ReadOnly
        } else {
            TeamRole::Member
        })
    }

    /// Whether `account` may write `team`'s apps: a member whose direct row
    /// there is not read-only.
    #[must_use]
    pub fn writes_team(&self, account: &str, team: &str) -> bool {
        self.in_team(account, team) && self.team_role(account, team) != Some(TeamRole::ReadOnly)
    }

    /// Whether `account` sees `team` at all (its name, members, metadata): a
    /// member, or someone who manages it. A restricted team is invisible to
    /// everyone else, and so is everything under it they do not reach.
    #[must_use]
    pub fn sees_team(&self, account: &str, team: &str) -> bool {
        self.in_team(account, team) || self.manages_team(account, team)
    }

    /// The organization-wide capabilities of `account` ([`caps`]).
    #[must_use]
    pub fn org_caps(&self, account: &str) -> u32 {
        if self.role(account).is_some_and(|r| r >= OrgRole::Admin) {
            return u32::MAX;
        }
        self.capabilities
            .get(account)
            .copied()
            .unwrap_or(self.default_capabilities)
    }

    /// Whether `bits` name only defined capabilities ([`caps::DEFINED`]).
    #[must_use]
    pub const fn defined_caps(bits: u32) -> bool {
        bits & !caps::DEFINED == 0
    }

    /// Whether `account` may hand out `bits`: admins any, others only bits
    /// they hold organization-wide.
    #[must_use]
    pub fn may_grant(&self, account: &str, bits: u32) -> bool {
        bits & !self.org_caps(account) == 0
    }

    /// Whether `account` holds `cap` for `team` (a team they manage, or their
    /// row's capabilities there, else the team's default while they are in it
    /// on top of their organization-wide ones), or organization-wide when
    /// `team` is `None`.
    #[must_use]
    pub fn may(&self, account: &str, team: Option<&str>, cap: u32) -> bool {
        match team {
            Some(t) if self.manages_team(account, t) => true,
            Some(t) => self.team_caps(account, t) & cap != 0,
            None => self.org_caps(account) & cap != 0,
        }
    }

    /// Whether `account` is a guest of an organization whose apps have app
    /// teams: they reach only the apps of the teams they have a row in (no
    /// organization-access app, no open top-level team). In a legacy
    /// organization, the organization is its one app, and a guest reaches its
    /// organization-access apps as before.
    #[must_use]
    pub fn app_only(&self, account: &str) -> bool {
        self.role(account) == Some(OrgRole::Guest) && self.mode() != OrgMode::Legacy
    }

    /// The members who get every app added from now on ([`Org::all_apps`]),
    /// as long as they are members.
    #[must_use]
    pub fn all_apps_members(&self) -> Vec<AccountId> {
        self.all_apps
            .iter()
            .filter(|a| self.members.contains_key(*a))
            .cloned()
            .collect()
    }

    /// The app team of `package` here, if it has one.
    #[must_use]
    pub fn app_team(&self, package: &str) -> Option<&Team> {
        self.teams
            .values()
            .find(|t| t.app_package() == Some(package))
    }

    /// The package whose app team `team` is or sits below (the
    /// [`Team::app_package`] of its top-level ancestor), and whether `team`
    /// is that app team itself; `None` for an unknown team or one under a
    /// top-level team that is no app team.
    #[must_use]
    pub fn package_of_team<'a>(&'a self, team: &'a str) -> Option<(&'a str, bool)> {
        let mut id = team;
        // Parents never form a cycle; the bound only keeps a broken state
        // from looping.
        for _ in 0..=self.teams.len() {
            let t = self.teams.get(id)?;
            match t.parent.as_deref() {
                Some(parent) => id = parent,
                None => return t.app_package().map(|p| (p, id == team)),
            }
        }
        None
    }

    /// The organization's mode, from its app teams.
    #[must_use]
    pub fn mode(&self) -> OrgMode {
        let mut apps = self.teams.values().filter(|t| t.app_package().is_some());
        let Some(first) = apps.next() else {
            return OrgMode::Legacy;
        };
        if std::iter::once(first)
            .chain(apps)
            .any(|t| t.join == TeamJoin::Invite)
        {
            OrgMode::Shared
        } else {
            OrgMode::Standalone
        }
    }

    /// Whether `viewer` sees `other` in the organization's member directory
    /// (`GET /orgs/{org}`): admins and the owner see everyone; a member sees
    /// everyone unless the organization is shared, and then the people of the
    /// teams they are in and its admins; a guest, who has only the apps of
    /// their teams, sees only the people of those teams. Everyone sees
    /// themselves.
    #[must_use]
    pub fn sees_member(&self, viewer: &str, other: &str) -> bool {
        let Some(role) = self.role(viewer) else {
            return false;
        };
        if viewer == other || role >= OrgRole::Admin {
            return true;
        }
        if role >= OrgRole::Member {
            if self.mode() != OrgMode::Shared {
                return true;
            }
            if self.role(other).is_some_and(|r| r >= OrgRole::Admin) {
                return true;
            }
        }
        self.teams
            .keys()
            .any(|t| self.in_team(viewer, t) && self.in_team(other, t))
    }

    /// `team` and every team below it.
    #[must_use]
    pub fn subtree(&self, team: &str) -> Vec<TeamId> {
        let mut out = vec![team.to_owned()];
        let mut i = 0;
        while i < out.len() && out.len() <= self.teams.len() {
            let cur = out[i].clone();
            out.extend(
                self.teams
                    .values()
                    .filter(|t| t.parent.as_deref() == Some(cur.as_str()))
                    .map(|t| t.team_id.clone()),
            );
            i += 1;
        }
        out
    }

    /// Take `accounts`' direct rows in every team below `team` (not `team`
    /// itself): who lost a team leaves the restricted teams under it (its
    /// private channels), where a row would keep them in.
    pub fn drop_rows_below(&mut self, team: &str, accounts: &[AccountId]) {
        if accounts.is_empty() {
            return;
        }
        for id in self.subtree(team).into_iter().skip(1) {
            if let Some(t) = self.teams.get_mut(&id) {
                for a in accounts {
                    let _prev = t.members.remove(a);
                    let _was = t.read_only.remove(a);
                    let _caps = t.capabilities.remove(a);
                }
            }
        }
    }

    /// Whether `account` may manage `team`: an org admin, the team's lead, or
    /// the lead of an ancestor team.
    #[must_use]
    pub fn manages_team(&self, account: &str, team: &str) -> bool {
        if self.role(account).is_some_and(|r| r >= OrgRole::Admin) {
            return true;
        }
        let mut cur = Some(team.to_owned());
        let mut hops = 0;
        while let Some(t) = cur {
            let Some(rec) = self.teams.get(&t) else { break };
            if rec.members.get(account) == Some(&true) {
                return true;
            }
            cur = rec.parent.clone();
            hops += 1;
            if hops > 32 {
                break;
            }
        }
        false
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Invite {
    pub org: OrgId,
    pub role: OrgRole,
    #[serde(default)]
    pub teams: Vec<TeamId>,
    pub expires_at: u64,
    pub uses_left: u32,
    /// Who made it: `CreateInvite`'s actor, set where it applies. Empty in
    /// invites from before, and left out then, so they write as they did.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_by: AccountId,
    /// When it was made (Unix seconds, the creating node's clock); 0 and left
    /// out in invites from before.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub created_at: u64,
    /// Whoever redeems it also gets every app added to the organization
    /// later ([`Org::all_apps`]): an admin's to mint. Left out when false, so
    /// an invite from before reads and writes as it did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all_apps: bool,
    /// The capability bits a new member gets as their own setting
    /// ([`Org::capabilities`]) when they redeem it, never more than its
    /// creator holds. Left out when none: the organization's default applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<u32>,
}

#[allow(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// Who receives an event (R9). Resolved when the event is sent to a
/// subscriber, against the system state of that moment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Audience {
    /// Everyone who reaches the app.
    All,
    /// The account that made the call.
    Caller,
    Accounts {
        accounts: Vec<AccountId>,
    },
    /// The members of a team of the app's organization.
    Team {
        team: TeamId,
    },
    /// Members, admins and the owner of the organization.
    OrgMembers {
        org: OrgId,
    },
    /// Admins and the owner of the organization.
    OrgAdmins {
        org: OrgId,
    },
}

impl From<calimero_app_host::Audience> for Audience {
    fn from(a: calimero_app_host::Audience) -> Self {
        use calimero_app_host::Audience as H;
        match a {
            H::All => Self::All,
            H::Caller => Self::Caller,
            H::Accounts(accounts) => Self::Accounts { accounts },
            H::Team(team) => Self::Team { team },
            H::OrgMembers(org) => Self::OrgMembers { org },
            H::OrgAdmins(org) => Self::OrgAdmins { org },
        }
    }
}

/// One event committed with a call.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EventRecord {
    pub index: u64,
    pub seq: u32,
    pub kind: String,
    pub data: serde_json::Value,
    /// A component's event: who receives it (R9), checked when it is sent.
    /// `None` for an event that goes to everyone who reaches the app (one an SDK
    /// 0.12 app recorded).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audience: Option<Audience>,
    /// The calling account, for [`Audience::Caller`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub caller: Option<AccountId>,
}

/// Seconds since the epoch.
#[must_use]
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The time an update decision is logged at (an upgrade's `at`, an undo's, the automatic
/// pass's): [`now_secs`], moved ahead in tests by
/// `crate::testing::shift_update_clock` so a hold or the undo window can pass
/// without waiting for it. Every rule judges the logged time, never a node's clock.
#[must_use]
pub fn update_now() -> u64 {
    #[cfg(feature = "testing")]
    {
        now_secs().saturating_add(crate::testing::update_clock_shift())
    }
    #[cfg(not(feature = "testing"))]
    {
        now_secs()
    }
}

/// Milliseconds since the epoch.
#[must_use]
pub fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// For an `Option<Option<T>>` field: absent is `None`, `null` is `Some(None)`
/// ("clear it"), a value is `Some(Some(v))`. Plain serde reads `null` as
/// absent, which silently dropped "move this team to the top".
pub fn present<'de, D, T>(d: D) -> Result<Option<Option<T>>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: serde::Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}
