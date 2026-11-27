//! Web origins a package's manager registers for its frontend
//! (`/package-origins/{package}`): a developer's own site (their domain, a
//! hosting preview) that signs people in to the cluster with passkeys.
//!
//! A passkey ceremony from a registered origin opens a session bound to the
//! package, as one from a frontend the node serves does (`SessionClaims::app`),
//! and names the origin (`SessionClaims::origin`), so removing the origin ends
//! those sessions. Each origin carries its own WebAuthn relying-party id
//! (`rpId`): the origin's host, or a parent domain of it. The cluster's own
//! rp-id is never one (its passkeys stay on the cluster's origins and the
//! frontends it serves), so a person has a passkey per developer site.
//!
//! The registrations live in the system state ([`SystemState::package_origins`]);
//! adding and removing one carries the manager's passkey consent
//! (`package:{package} / __add_origin / { origin, rpId }`,
//! `__remove_origin / { origin }`), checked where it applies.
//!
//! An origin may instead be registered for one app instance by those who
//! manage it (`/apps/{app}/origins`, `{app} / __add_origin`): its sessions are
//! bound to the instance's package and reach that instance only
//! ([`PackageOrigin::app`]), as a store's own site does.
//!
//! A **served** origin (`served: true`, a custom domain) is one the cluster
//! also serves the package's frontend on, from its signed bundle, as it does
//! at `{label}.{apps_domain}` (`app_host`): the developer points their domain
//! at the cluster and the node answers it. Everything else is a registered
//! origin's: its own rp-id, sessions bound to the package and naming the
//! origin, removal ending them. Its consent says so
//! (`{ origin, rpId, served: true }`), so a consent to a plain origin never
//! makes one served. Design: [Custom domains](https://calimero-network.github.io/core/protocol/applications/#custom-domains).

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::system::cluster_version::Feature;
use crate::system::{Rejection, SystemState, OPERATOR};
use crate::types::{AccountId, AppId};

/// How many origins one package registers.
pub const MAX_ORIGINS_PER_PACKAGE: usize = 8;

/// How many origins one app instance registers.
pub const MAX_ORIGINS_PER_APP: usize = 4;

/// How many packages' origins a cluster keeps.
pub const MAX_PACKAGE_ORIGINS: usize = 4096;

/// How many instances' origins a cluster keeps, apart from packages' own.
pub const MAX_APP_ORIGINS: usize = 4096;

/// How many instances' origins one organization's instances register: a
/// sixty-fourth of [`MAX_APP_ORIGINS`], so no one organization fills it.
pub const MAX_APP_ORIGINS_PER_ORG: usize = 64;

/// Refuses an entry naming no [`PackageOrigin::registration`], which no re-registration would change.
const NO_REGISTRATION: &str = "an origin's registration is empty";
/// Refuses taking back an instance's origin for a package no publisher is bound to.
pub const INSTANCE_HOLDS_ORIGIN: &str =
    "an instance of the package registered that origin; only its bound publisher takes it back";

/// The longest origin, in bytes (`scheme://host:port`).
pub const MAX_ORIGIN_BYTES: usize = 255;

/// How many of a package's origins the cluster serves its frontend on
/// (custom domains); each costs a certificate and a route outside the node.
pub const MAX_SERVED_ORIGINS_PER_PACKAGE: usize = 4;

/// How many served origins a cluster keeps: every node holds a certificate
/// chain for each, delivered as instance metadata (a few KiB each, within
/// the 512 KiB a GCE instance's metadata may hold, beside the node's own).
pub const MAX_SERVED_ORIGINS: usize = 64;

/// One registered origin.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PackageOrigin {
    /// The package whose sessions a ceremony here opens.
    pub package: String,
    /// The WebAuthn relying-party id ceremonies here are verified against.
    pub rp_id: String,
    pub added_by: AccountId,
    pub added_at: u64,
    /// Whether the cluster serves the package's frontend on this origin (a
    /// custom domain). Left out when false, so a registration from before
    /// reads and writes as it did.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub served: bool,
    /// The instance its sessions reach, alone; `None` for the package's own
    /// origin, whose sessions reach every instance of the package.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppId>,
    /// Random, from the entry that registered it (empty in one from before): a
    /// session names it, so the origin removed and registered again ends it.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub registration: String,
}

/// A new registration's id ([`PackageOrigin::registration`]).
#[must_use]
pub fn new_registration() -> String {
    calimero_identity::b64url(&calimero_identity::random::<16>())
}

/// Why an origin is refused before anything is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not `https://host[:port]` written as a browser serializes it (or
    /// `http://` on a `localhost` name).
    Origin,
    /// Not the origin's host, nor a parent domain of it.
    RpId,
    /// A served origin with a port (or `http` with none): browsers reach a
    /// custom domain on 443 only.
    Served,
}

impl Refusal {
    /// What the refusal says, at the API and at apply.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Origin => "an origin is https://host[:port] exactly as a browser writes it (lowercase, no path, no default port); http only on a localhost name",
            Self::RpId => "an rpId is the origin's host or a parent domain of it, with at least one dot (or localhost)",
            Self::Served => "a served origin is https://host with no port (http://name.localhost:port on a development node)",
        }
    }
}

/// Whether `host` is `localhost` or a name under it.
fn is_localhost(host: &str) -> bool {
    host == "localhost" || host.ends_with(".localhost")
}

/// The host of `origin`, when it is one an origin may be: exactly the
/// browser's serialization of `https://host[:port]` (or `http://` for a
/// `localhost` name), a domain name rather than an address.
///
/// # Errors
/// [`Refusal::Origin`].
pub fn origin_host(origin: &str) -> Result<String, Refusal> {
    if origin.len() > MAX_ORIGIN_BYTES {
        return Err(Refusal::Origin);
    }
    let url = url::Url::parse(origin).map_err(|_| Refusal::Origin)?;
    // Anything the serialization drops (a path, a query, credentials, a
    // default port, upper case) makes another string: one spelling per origin.
    if url.origin().ascii_serialization() != origin {
        return Err(Refusal::Origin);
    }
    let Some(url::Host::Domain(host)) = url.host() else {
        return Err(Refusal::Origin);
    };
    let fits = match url.scheme() {
        "https" => true,
        "http" => is_localhost(host),
        _ => false,
    };
    if !fits {
        return Err(Refusal::Origin);
    }
    Ok(host.to_owned())
}

/// The host of `origin` when the cluster may serve a frontend on it: an
/// origin [`origin_host`] takes, `https` with no port (browsers reach it on
/// 443, through the resolver), or `http` on a `localhost` name with one.
///
/// # Errors
/// [`Refusal::Origin`], [`Refusal::Served`].
pub fn served_host(origin: &str) -> Result<String, Refusal> {
    let host = origin_host(origin)?;
    let rest = origin
        .split_once("://")
        .map_or(origin, |(_, rest)| rest)
        .strip_prefix(host.as_str())
        .unwrap_or_default();
    let fits = if origin.starts_with("https://") {
        rest.is_empty()
    } else {
        !rest.is_empty()
    };
    if !fits {
        return Err(Refusal::Served);
    }
    Ok(host)
}

/// The arguments the manager's consent to `AddPackageOrigin` names:
/// `{ origin, rpId }`, plus `served: true` for a served one, so either
/// consent fits only its own change.
#[must_use]
pub fn add_args(origin: &str, rp_id: &str, served: bool) -> Value {
    if served {
        json!({ "origin": origin, "rpId": rp_id, "served": true })
    } else {
        json!({ "origin": origin, "rpId": rp_id })
    }
}

/// Whether `name` is `domain` or a name under it (both lowercase, no
/// trailing dot).
#[must_use]
pub fn within(name: &str, domain: &str) -> bool {
    name == domain
        || name
            .strip_suffix(domain)
            .is_some_and(|rest| rest.ends_with('.'))
}

/// The rp-id ceremonies on `origin` use: `rp_id` when given, else the
/// origin's host. It must be the host or a parent domain of it, as WebAuthn
/// requires of a relying party; a browser also refuses a public suffix.
///
/// # Errors
/// The origin or the rp-id does not fit.
pub fn resolve_rp_id(origin: &str, rp_id: Option<&str>) -> Result<String, Refusal> {
    let host = origin_host(origin)?;
    let Some(rp) = rp_id else {
        return Ok(host);
    };
    let label_suffix = host == rp
        || host
            .strip_suffix(rp)
            .is_some_and(|rest| rest.ends_with('.'));
    let named = rp == "localhost" || (rp.contains('.') && !rp.starts_with('.'));
    if !label_suffix || !named || rp.bytes().any(|b| b.is_ascii_uppercase()) {
        return Err(Refusal::RpId);
    }
    Ok(rp.to_owned())
}

/// Whether `rp_id` is a public suffix (`co.uk`, `github.io`), which a browser refuses as an rp-id.
/// Checked at the API only: the list changes between releases, so apply never reads it.
#[must_use]
pub fn is_public_suffix(rp_id: &str) -> bool {
    rp_id != "localhost" && psl::domain_str(rp_id).is_none()
}

/// Whether `rp_id` is `cluster_rp_id` or a parent domain of it: one whose
/// ceremonies the cluster's own passkeys could answer. `localhost` (a
/// development cluster) is everyone's.
#[must_use]
pub fn covers_cluster(rp_id: &str, cluster_rp_id: &str) -> bool {
    if rp_id == "localhost" {
        return false;
    }
    within(cluster_rp_id, rp_id)
}

impl SystemState {
    /// Whether `account` manages `package`'s origins: where its signing key
    /// is bound to a publisher organization, that organization's org-wide
    /// `MANAGE_APPLICATION` holders; else the account that installed its
    /// first release. Never the operator.
    #[must_use]
    pub fn manages_package(&self, package: &str, account: &str) -> bool {
        if account == OPERATOR {
            return false;
        }
        let Some(p) = self.packages.get(package) else {
            return false;
        };
        match self.publisher_of_signer(&p.signer_id) {
            Some(org) => self.manages_apps(org, account, None),
            // A first installer, only while a release of it is theirs to use (from the
            // version that brings private releases, so earlier entries replay as they applied).
            None => {
                p.owner == account
                    && (!self
                        .cluster_brings(crate::system::cluster_version::Feature::PrivateReleases)
                        || p.versions.values().any(|v| self.sees_release(v, account)))
            }
        }
    }

    /// Whether `package`'s manager takes back an origin an instance of it holds:
    /// only where a publisher is bound, never on a first installer's word.
    #[must_use]
    pub fn reclaims_instance_origins(&self, package: &str) -> bool {
        self.packages
            .get(package)
            .is_some_and(|p| self.publisher_of_signer(&p.signer_id).is_some())
    }

    /// The origins registered for `package` by its manager, by origin.
    pub fn origins_of<'a>(
        &'a self,
        package: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a PackageOrigin)> + 'a {
        self.package_origins
            .iter()
            .filter(move |(_, o)| o.package == package && o.app.is_none())
    }

    /// Whether the cluster holds as many instances' (`instance`) or packages' origins as it takes.
    pub(crate) fn origins_full(&self, instance: bool) -> bool {
        let cap = if instance {
            MAX_APP_ORIGINS
        } else {
            MAX_PACKAGE_ORIGINS
        };
        self.package_origins
            .values()
            .filter(|o| o.app.is_some() == instance)
            .count()
            >= cap
    }

    /// Whether the instances of `org` hold as many origins as one organization's take.
    pub(crate) fn org_origins_full(&self, org: &str) -> bool {
        self.package_origins
            .values()
            .filter_map(|o| o.app.as_ref())
            .filter(|app| self.apps.get(app.as_str()).is_some_and(|r| r.org == org))
            .count()
            >= MAX_APP_ORIGINS_PER_ORG
    }

    /// The origins registered for app instance `app`, by origin.
    pub fn origins_of_app<'a>(
        &'a self,
        app: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a PackageOrigin)> + 'a {
        self.package_origins
            .iter()
            .filter(move |(_, o)| o.app.as_deref() == Some(app))
    }

    /// The registration whose origin the cluster serves `package`'s
    /// frontend on, for an origin (`scheme://host[:port]`).
    #[must_use]
    pub fn served_origin(&self, origin: &str) -> Option<&PackageOrigin> {
        self.package_origins.get(origin).filter(|o| o.served)
    }

    /// `AddPackageOrigin`: by the package's manager, an origin no package
    /// holds yet, within the limits; `served`, one the cluster serves the
    /// package's frontend on.
    #[allow(clippy::too_many_arguments, reason = "the command's fields")]
    pub(crate) fn add_package_origin(
        &mut self,
        package: String,
        origin: String,
        rp_id: &str,
        served: bool,
        registration: String,
        actor: AccountId,
        at: u64,
    ) -> Result<Value, Rejection> {
        if !self.manages_package(&package, &actor) {
            return Err("only the package's manager registers its origins: its publisher's MANAGE_APPLICATION holders, or whoever installed it first".into());
        }
        if registration.is_empty() && self.cluster_brings(Feature::AppOrigins) {
            return Err(NO_REGISTRATION.into());
        }
        let resolved = resolve_rp_id(&origin, Some(rp_id)).map_err(Refusal::message)?;
        if served {
            let _host = served_host(&origin).map_err(Refusal::message)?;
        }
        if self.package_origins.get(&origin).is_some_and(|o| {
            o.app.is_some() && o.package == package && !self.reclaims_instance_origins(&package)
        }) {
            return Err(INSTANCE_HOLDS_ORIGIN.into());
        }
        // One an instance of the package holds, a bound publisher's manager takes back.
        if let Some(held) = self
            .package_origins
            .get(&origin)
            .filter(|o| o.package != package || o.app.is_none())
        {
            if held.package != package {
                return Err("another package registered that origin".into());
            }
            if held.rp_id != resolved {
                return Err("that origin is registered with another rpId; remove it first".into());
            }
            if held.served != served {
                return Err(
                    "that origin is registered with the other `served`; remove it first".into(),
                );
            }
            return Ok(json!(origin_json(&origin, held)));
        }
        if self.origins_of(&package).count() >= MAX_ORIGINS_PER_PACKAGE {
            return Err(Rejection::quota("origins per package"));
        }
        if served
            && self.origins_of(&package).filter(|(_, o)| o.served).count()
                >= MAX_SERVED_ORIGINS_PER_PACKAGE
        {
            return Err(Rejection::quota("served origins per package"));
        }
        if served
            && self.package_origins.values().filter(|o| o.served).count() >= MAX_SERVED_ORIGINS
        {
            return Err(Rejection::quota("served origins"));
        }
        if self.origins_full(false) {
            return Err(Rejection::quota("origins"));
        }
        let record = PackageOrigin {
            package,
            rp_id: resolved,
            added_by: actor,
            added_at: at,
            served,
            app: None,
            registration,
        };
        let out = json!(origin_json(&origin, &record));
        let _prev = self.package_origins.insert(origin, record);
        Ok(out)
    }

    /// `RemovePackageOrigin`: by the package's manager.
    pub(crate) fn remove_package_origin(
        &mut self,
        package: &str,
        origin: &str,
        actor: &str,
    ) -> Result<Value, Rejection> {
        if !self.manages_package(package, actor) {
            return Err("only the package's manager removes its origins".into());
        }
        if self
            .package_origins
            .get(origin)
            .is_none_or(|o| o.package != package || o.app.is_some())
        {
            return Err("that origin is not registered for this package".into());
        }
        let _gone = self.package_origins.remove(origin);
        Ok(json!(OriginRemoved {
            package: package.to_owned(),
            origin: origin.to_owned(),
            removed: true,
        }))
    }

    /// `AddAppOrigin`: by a manager of instance `app`, an origin no one holds
    /// yet, within the limits; its sessions are bound to the package the
    /// instance runs and reach the instance alone.
    pub(crate) fn add_app_origin(
        &mut self,
        app: AppId,
        origin: String,
        rp_id: &str,
        registration: String,
        actor: AccountId,
        at: u64,
    ) -> Result<Value, Rejection> {
        let record = self.apps.get(&app).ok_or("no such app")?;
        if !self.manages_app(record, &actor) {
            return Err("only whoever manages the instance registers its origins".into());
        }
        if registration.is_empty() {
            return Err(NO_REGISTRATION.into());
        }
        // The release the instance runs, as `Principal::within` judges it: a
        // shared component bundle's record names only its first package.
        let package = self
            .running_release(record)
            .filter(|r| {
                self.packages
                    .get(&r.package)
                    .is_some_and(|p| p.versions.contains_key(&r.version))
            })
            .ok_or("the instance runs no installed release")?
            .package;
        let resolved = resolve_rp_id(&origin, Some(rp_id)).map_err(Refusal::message)?;
        if let Some(held) = self.package_origins.get(&origin) {
            if held.app.as_deref() != Some(app.as_str()) {
                return Err("another app or package registered that origin".into());
            }
            if held.rp_id != resolved {
                return Err("that origin is registered with another rpId; remove it first".into());
            }
            return Ok(json!(origin_json(&origin, held)));
        }
        if self.origins_of_app(&app).count() >= MAX_ORIGINS_PER_APP {
            return Err(Rejection::quota("origins per app"));
        }
        if self
            .apps
            .get(&app)
            .is_some_and(|r| self.org_origins_full(&r.org))
        {
            return Err(Rejection::quota("instance origins per organization"));
        }
        if self.origins_full(true) {
            return Err(Rejection::quota("origins"));
        }
        let record = PackageOrigin {
            package,
            rp_id: resolved,
            added_by: actor,
            added_at: at,
            served: false,
            app: Some(app),
            registration,
        };
        let out = json!(origin_json(&origin, &record));
        let _prev = self.package_origins.insert(origin, record);
        Ok(out)
    }

    /// `RemoveAppOrigin`: by a manager of instance `app`.
    pub(crate) fn remove_app_origin(
        &mut self,
        app: &str,
        origin: &str,
        actor: &str,
    ) -> Result<Value, Rejection> {
        let record = self.apps.get(app).ok_or("no such app")?;
        if !self.manages_app(record, actor) {
            return Err("only whoever manages the instance removes its origins".into());
        }
        let Some(held) = self
            .package_origins
            .get(origin)
            .filter(|o| o.app.as_deref() == Some(app))
        else {
            return Err("that origin is not registered for this app".into());
        };
        let package = held.package.clone();
        let _gone = self.package_origins.remove(origin);
        Ok(json!(OriginRemoved {
            package,
            origin: origin.to_owned(),
            removed: true,
        }))
    }

    /// `origin`'s registration a session of `package` opened under
    /// `registration` names, while it holds: none once removed or registered anew.
    pub(crate) fn origin_registration(
        &self,
        origin: &str,
        package: Option<&str>,
        registration: &str,
    ) -> Option<&PackageOrigin> {
        self.package_origins
            .get(origin)
            .filter(|o| Some(o.package.as_str()) == package && o.registration == registration)
    }

    /// Forget the origins of a package no longer installed, so whoever
    /// installs a package of that name again does not inherit them.
    pub(crate) fn forget_origins(&mut self, package: &str) {
        self.package_origins.retain(|_, o| o.package != package);
    }

    /// Forget instance `app`'s origins added under a package it no longer runs:
    /// their sessions are bound to that package and reach the instance no more.
    pub(crate) fn forget_moved_app_origins(&mut self, app: &str) {
        let package = self
            .apps
            .get(app)
            .and_then(|r| self.running_release(r))
            .map(|r| r.package);
        self.package_origins
            .retain(|_, o| o.app.as_deref() != Some(app) || package.as_ref() == Some(&o.package));
    }

    /// Forget instance `app`'s origins, which were its managers' until it was
    /// deleted, moved or handed on, and with them the sessions opened there.
    pub(crate) fn forget_app_origins(&mut self, app: &str) {
        self.package_origins
            .retain(|_, o| o.app.as_deref() != Some(app));
    }
}

/// What removing an origin answers.
#[derive(Debug, Serialize, Deserialize)]
pub struct OriginRemoved {
    pub package: String,
    pub origin: String,
    pub removed: bool,
}

/// A registration as the API answers it (`#PackageOrigin`).
#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OriginView {
    pub package: String,
    pub origin: String,
    pub rp_id: String,
    pub added_by: AccountId,
    /// RFC 3339.
    pub added_at: String,
    pub served: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<AppId>,
}

/// `o`, registered for `origin`, as the API answers it.
#[must_use]
pub fn origin_json(origin: &str, o: &PackageOrigin) -> OriginView {
    OriginView {
        package: o.package.clone(),
        origin: origin.to_owned(),
        rp_id: o.rp_id.clone(),
        added_by: o.added_by.clone(),
        added_at: crate::system::rfc3339(o.added_at),
        served: o.served,
        app: o.app.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_written_one_way() {
        assert_eq!(
            origin_host("https://app.example.com").unwrap(),
            "app.example.com"
        );
        assert_eq!(
            origin_host("https://app.example.com:8443").unwrap(),
            "app.example.com"
        );
        assert_eq!(origin_host("http://localhost:3000").unwrap(), "localhost");
        assert_eq!(
            origin_host("http://my.localhost:3000").unwrap(),
            "my.localhost"
        );
        for bad in [
            "http://app.example.com",
            "https://app.example.com/",
            "https://app.example.com/path",
            "https://App.example.com",
            "https://app.example.com:443",
            "https://user@app.example.com",
            "https://app.example.com?x=1",
            "https://127.0.0.1",
            "https://[::1]",
            "ftp://app.example.com",
            "app.example.com",
            "",
        ] {
            assert_eq!(origin_host(bad), Err(Refusal::Origin), "{bad}");
        }
        let long = format!("https://{}.com", "a".repeat(MAX_ORIGIN_BYTES));
        assert_eq!(origin_host(&long), Err(Refusal::Origin));
    }

    #[test]
    fn an_rp_id_is_the_host_or_a_parent_domain() {
        let o = "https://app.example.com";
        assert_eq!(resolve_rp_id(o, None).unwrap(), "app.example.com");
        assert_eq!(
            resolve_rp_id(o, Some("app.example.com")).unwrap(),
            "app.example.com"
        );
        assert_eq!(
            resolve_rp_id(o, Some("example.com")).unwrap(),
            "example.com"
        );
        for bad in [
            "com",
            "ample.com",
            "other.com",
            ".example.com",
            "Example.com",
            "pp.example.com",
        ] {
            assert_eq!(resolve_rp_id(o, Some(bad)), Err(Refusal::RpId), "{bad}");
        }
        let o = "https://app.example.co.uk";
        assert_eq!(
            resolve_rp_id(o, Some("example.co.uk")).unwrap(),
            "example.co.uk"
        );
        // Apply takes a public suffix: only the API consults the list.
        assert_eq!(resolve_rp_id(o, Some("co.uk")).unwrap(), "co.uk");
        assert_eq!(
            resolve_rp_id("http://localhost:3000", Some("localhost")).unwrap(),
            "localhost"
        );
        assert_eq!(
            resolve_rp_id("http://my.localhost:3000", Some("localhost")).unwrap(),
            "localhost"
        );
    }

    /// A custom domain is reached on 443 through the resolver, so its origin
    /// has no port; only a development node's `localhost` name has one (and
    /// `http`).
    #[test]
    fn a_served_origin_is_an_https_host_without_a_port() {
        assert_eq!(
            served_host("https://kv.example.com").unwrap(),
            "kv.example.com"
        );
        assert_eq!(
            served_host("http://kv.localhost:2501").unwrap(),
            "kv.localhost"
        );
        for bad in ["https://kv.example.com:8443", "http://kv.localhost"] {
            assert_eq!(served_host(bad), Err(Refusal::Served), "{bad}");
        }
        for bad in ["http://kv.example.com", "https://kv.example.com/", ""] {
            assert_eq!(served_host(bad), Err(Refusal::Origin), "{bad}");
        }
    }

    /// The consent to a served origin names `served`, so the one to a plain
    /// origin (as the clients sign it today) never stands for it.
    #[test]
    fn a_served_origins_consent_names_it_served() {
        let plain = add_args("https://kv.example.com", "kv.example.com", false);
        assert_eq!(
            plain,
            json!({ "origin": "https://kv.example.com", "rpId": "kv.example.com" })
        );
        let served = add_args("https://kv.example.com", "kv.example.com", true);
        assert_eq!(served["served"], true);
        assert_ne!(plain, served);
    }

    #[test]
    fn a_public_suffix_is_no_rp_id() {
        for suffix in ["co.uk", "github.io", "com"] {
            assert!(is_public_suffix(suffix), "{suffix}");
        }
        for name in ["example.co.uk", "kv.github.io", "localhost", "kv.localhost"] {
            assert!(!is_public_suffix(name), "{name}");
        }
    }

    #[test]
    fn within_matches_whole_labels_only() {
        assert!(within("kv.example.com", "example.com"));
        assert!(within("example.com", "example.com"));
        assert!(!within("kvexample.com", "example.com"));
        assert!(!within("example.com", "kv.example.com"));
    }

    #[test]
    fn the_clusters_rp_id_and_its_parents_cover_the_cluster() {
        assert!(covers_cluster("calimero.network", "calimero.network"));
        assert!(covers_cluster("calimero.network", "id.calimero.network"));
        assert!(!covers_cluster(
            "app.calimero.network",
            "id.calimero.network"
        ));
        assert!(!covers_cluster("example.com", "calimero.network"));
        assert!(!covers_cluster("work.com", "network.com"));
        assert!(!covers_cluster("localhost", "localhost"));
    }
}
