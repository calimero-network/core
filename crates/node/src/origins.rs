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

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::system::{SystemState, OPERATOR};
use crate::types::AccountId;

/// How many origins one package registers.
pub const MAX_ORIGINS_PER_PACKAGE: usize = 8;

/// How many origins a cluster keeps.
pub const MAX_PACKAGE_ORIGINS: usize = 4096;

/// The longest origin, in bytes (`scheme://host:port`).
pub const MAX_ORIGIN_BYTES: usize = 255;

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
}

/// Why an origin is refused before anything is recorded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// Not `https://host[:port]` written as a browser serializes it (or
    /// `http://` on a `localhost` name).
    Origin,
    /// Not the origin's host, nor a parent domain of it.
    RpId,
}

impl Refusal {
    /// What the refusal says, at the API and at apply.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::Origin => "an origin is https://host[:port] exactly as a browser writes it (lowercase, no path, no default port); http only on a localhost name",
            Self::RpId => "an rpId is the origin's host or a parent domain of it, with at least one dot (or localhost)",
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

/// Whether `rp_id` is `cluster_rp_id` or a parent domain of it: one whose
/// ceremonies the cluster's own passkeys could answer. `localhost` (a
/// development cluster) is everyone's.
#[must_use]
pub fn covers_cluster(rp_id: &str, cluster_rp_id: &str) -> bool {
    if rp_id == "localhost" {
        return false;
    }
    cluster_rp_id == rp_id
        || cluster_rp_id
            .strip_suffix(rp_id)
            .is_some_and(|rest| rest.ends_with('.'))
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
            None => p.owner == account,
        }
    }

    /// The origins registered for `package`, by origin.
    pub fn origins_of<'a>(
        &'a self,
        package: &'a str,
    ) -> impl Iterator<Item = (&'a String, &'a PackageOrigin)> + 'a {
        self.package_origins
            .iter()
            .filter(move |(_, o)| o.package == package)
    }

    /// `AddPackageOrigin`: by the package's manager, an origin no package
    /// holds yet, within the limits.
    pub(crate) fn add_package_origin(
        &mut self,
        package: String,
        origin: String,
        rp_id: &str,
        actor: AccountId,
        at: u64,
    ) -> Result<Value, &'static str> {
        if !self.manages_package(&package, &actor) {
            return Err("only the package's manager registers its origins: its publisher's MANAGE_APPLICATION holders, or whoever installed it first");
        }
        let resolved = resolve_rp_id(&origin, Some(rp_id)).map_err(Refusal::message)?;
        if let Some(held) = self.package_origins.get(&origin) {
            if held.package != package {
                return Err("another package registered that origin");
            }
            if held.rp_id != resolved {
                return Err("that origin is registered with another rpId; remove it first");
            }
            return Ok(origin_json(&origin, held));
        }
        if self.origins_of(&package).count() >= MAX_ORIGINS_PER_PACKAGE {
            return Err("quota exceeded: origins per package");
        }
        if self.package_origins.len() >= MAX_PACKAGE_ORIGINS {
            return Err("quota exceeded: origins");
        }
        let record = PackageOrigin {
            package,
            rp_id: resolved,
            added_by: actor,
            added_at: at,
        };
        let out = origin_json(&origin, &record);
        let _prev = self.package_origins.insert(origin, record);
        Ok(out)
    }

    /// `RemovePackageOrigin`: by the package's manager.
    pub(crate) fn remove_package_origin(
        &mut self,
        package: &str,
        origin: &str,
        actor: &str,
    ) -> Result<Value, &'static str> {
        if !self.manages_package(package, actor) {
            return Err("only the package's manager removes its origins");
        }
        if self
            .package_origins
            .get(origin)
            .is_none_or(|o| o.package != package)
        {
            return Err("that origin is not registered for this package");
        }
        let _gone = self.package_origins.remove(origin);
        Ok(json!({ "package": package, "origin": origin, "removed": true }))
    }

    /// Forget the origins of a package no longer installed, so whoever
    /// installs a package of that name again does not inherit them.
    pub(crate) fn forget_origins(&mut self, package: &str) {
        self.package_origins.retain(|_, o| o.package != package);
    }
}

/// A registration as the API answers it.
#[must_use]
pub fn origin_json(origin: &str, o: &PackageOrigin) -> Value {
    json!({
        "package": o.package,
        "origin": origin,
        "rpId": o.rp_id,
        "addedBy": o.added_by,
        "addedAt": crate::system::rfc3339(o.added_at),
    })
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
        assert_eq!(
            resolve_rp_id("http://localhost:3000", Some("localhost")).unwrap(),
            "localhost"
        );
        assert_eq!(
            resolve_rp_id("http://my.localhost:3000", Some("localhost")).unwrap(),
            "localhost"
        );
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
