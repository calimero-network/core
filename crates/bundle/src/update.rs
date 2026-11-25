//! Whether an update of an app instance changes where its data can go: **green** or
//! **red**, judged from the signed manifests and embedded ABIs of the releases it runs and
//! would run, over its whole unit (the app and every app it created with `uses` in mode
//! `create`, transitively).
//!
//! The one classifier the node (`GET /apps/{app}/update`, the consent page's upgrade
//! sheet) and the command line (an app's check against an earlier release) call: pure, so
//! both say the same of the same releases.
//!
//! ## Egress
//!
//! A release's egress is the set of destinations its runs may send a request to: its
//! `outbound` prefixes, and the host each secret it declares is bound to (`https://<host>/`)
//! where no prefix already names that host (a manifest the node installs never has one:
//! [`BundleManifest::check_secrets`] refuses a host no prefix covers). A unit's egress is
//! the union over the app and its members. A destination is covered by another when it is
//! the same `https://` authority and its path is the other's or below it at a `/`
//! ([`covers`], the rule `calimero-outbound` applies to each request).
//!
//! An empty egress means no data leaves Calimero. Any destination outside the cluster is
//! just a destination, whatever runs there: the platform never vouches for an external
//! endpoint, a TEE (such as a private-inference service) included. An app that wants to
//! trust one verifies its attestation in its own code; it is still listed.
//!
//! ## Red and green
//!
//! An update is **red**, and so never applies without a person's approval, when any of
//! these holds ([`Reason`]):
//!
//! - **`signerChanged`**: the app or a member that is in the unit on both sides is signed
//!   by another key: the code is no longer from whoever was trusted with it.
//! - **`egressAdded`**: a destination after the update is covered by none before (members
//!   counted): data may go somewhere it could not.
//! - **`stateVersionChanged`**: the app or a member declares another state version
//!   (`#[app::state(version = N)]`; a side whose component embeds no ABI counts as a
//!   version of its own): an update runs no migration, so the new code reads data the old
//!   one left in a shape it may not expect.
//! - **`memberAdded`** / **`memberRemoved`**: an app joins or leaves the unit while its
//!   release has egress: a joining member brings destinations of its own, and one that
//!   leaves is no longer upgraded with the unit, so where its data goes leaves this
//!   consent. A member without egress joins or leaves green.
//! - **`delegatedMethodsAdded`**: a release calls a method of another app as the person
//!   (`uses.<name>.methods`) it did not call as them before: a new way to act as the user.
//! - **`secretBindingChanged`**: a release declares a secret it did not, or binds one to
//!   another host, header, format or body field, or to a path its earlier binding did not
//!   cover ([`crate::BundleSecret`]): a credential someone supplied would be sent somewhere,
//!   or in a way, they did not agree to. A binding narrowed to fewer paths stays green.
//!
//! Otherwise it is **green**: the same signers, egress the same or smaller, the same state
//! versions, the same credential bindings or fewer, and no new way to act as the person. New and removed methods are reported
//! (`newMethods`, `removedMethods`) but never make an update red on their own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::{is_outbound_url, BundleManifest, BundleSecret};

/// What the classifier reads of one release: its identity, where its runs may send
/// requests, how it acts as the person, and its ABI's state version and methods.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Release {
    pub package: String,
    pub version: String,
    /// The publisher key that signed the manifest (`did:key`).
    pub signer: String,
    /// The manifest's `outbound` prefixes.
    #[serde(default)]
    pub outbound: BTreeSet<String>,
    /// Each declared secret's binding, by secret name.
    #[serde(default)]
    pub secrets: BTreeMap<String, BundleSecret>,
    /// The methods this release calls as the person, by the package of the app it calls
    /// (`uses.<name>.methods`, every reference to that package together).
    #[serde(default)]
    pub delegated: BTreeMap<String, BTreeSet<String>>,
    /// The state version its component's ABI declares; `None` when it declares none or
    /// embeds no ABI.
    #[serde(default)]
    pub state_version: Option<u64>,
    /// The methods its component's ABI declares; `None` when it embeds no ABI.
    #[serde(default)]
    pub methods: Option<BTreeSet<String>>,
}

impl Release {
    /// A release's facts from its signed `manifest`, the key that signed it and the
    /// `calimero:abi` JSON its service's component embeds (`None` when it embeds none).
    #[must_use]
    pub fn new(manifest: &BundleManifest, signer: &str, abi: Option<&serde_json::Value>) -> Self {
        let mut delegated: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for u in manifest.uses.iter().flat_map(BTreeMap::values) {
            if !u.methods.is_empty() {
                delegated
                    .entry(u.package.clone())
                    .or_default()
                    .extend(u.methods.iter().cloned());
            }
        }
        Self {
            package: manifest.package.clone(),
            version: manifest.app_version.clone(),
            signer: signer.to_owned(),
            outbound: manifest.outbound.iter().cloned().collect(),
            secrets: manifest.secrets.clone().unwrap_or_default(),
            delegated,
            state_version: abi.and_then(|a| a["state"]["version"].as_u64()),
            methods: abi.map(|a| {
                a["methods"]
                    .as_object()
                    .map(|m| m.keys().cloned().collect())
                    .unwrap_or_default()
            }),
        }
    }

    /// Where this release's runs may send requests: its `outbound` prefixes, and
    /// `https://<host>/` for a secret's host no prefix names.
    #[must_use]
    pub fn egress(&self) -> BTreeSet<String> {
        let named: BTreeSet<String> = self
            .outbound
            .iter()
            .filter_map(|p| Destination::parse(p).map(|d| d.authority))
            .collect();
        let mut out = self.outbound.clone();
        for secret in self.secrets.values() {
            let host = secret.host.to_ascii_lowercase();
            if !named.contains(&host) {
                let _new = out.insert(format!("https://{host}/"));
            }
        }
        out
    }
}

/// An app and the members of its unit, each with the release it runs (or would run).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Unit {
    pub app: Release,
    /// Each member by its path from the app: its `uses` name, and for a member's own
    /// member `<name>/<name>`.
    #[serde(default)]
    pub members: BTreeMap<String, Release>,
}

impl Unit {
    /// A unit of `app` alone.
    #[must_use]
    pub const fn of(app: Release) -> Self {
        Self {
            app,
            members: BTreeMap::new(),
        }
    }

    /// The union of the app's and every member's egress ([`Release::egress`]).
    #[must_use]
    pub fn egress(&self) -> BTreeSet<String> {
        std::iter::once(&self.app)
            .chain(self.members.values())
            .flat_map(Release::egress)
            .collect()
    }

    /// The app (`""`) and each member by path, with its release.
    fn parts(&self) -> impl Iterator<Item = (&str, &Release)> {
        std::iter::once(("", &self.app)).chain(self.members.iter().map(|(k, r)| (k.as_str(), r)))
    }
}

/// An `https://` prefix taken apart: its authority (lowercase, without the default port)
/// and its path (`/` when it names none).
struct Destination {
    authority: String,
    path: String,
}

impl Destination {
    fn parse(url: &str) -> Option<Self> {
        if !is_outbound_url(url) {
            return None;
        }
        let rest = url.strip_prefix("https://")?;
        let (authority, path) = rest.find('/').map_or((rest, "/"), |at| rest.split_at(at));
        let authority = authority.to_ascii_lowercase();
        let authority = authority
            .strip_suffix(":443")
            .map_or_else(|| authority.clone(), str::to_owned);
        Some(Self {
            authority,
            path: path.to_owned(),
        })
    }
}

/// Whether the `https://` prefix `prefix` covers `url` (a URL or another prefix): the
/// same authority, and a path equal to the prefix's or below it at a `/`. Anything that
/// is not an `outbound` prefix ([`is_outbound_url`]) covers and is covered by nothing.
#[must_use]
pub fn covers(prefix: &str, url: &str) -> bool {
    let (Some(p), Some(u)) = (Destination::parse(prefix), Destination::parse(url)) else {
        return false;
    };
    if p.authority != u.authority {
        return false;
    }
    let (base, path) = (p.path.as_str(), u.path.as_str());
    path == base
        || (base.ends_with('/') && path.starts_with(base))
        || path
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Green: the update may apply under a policy that allows it; red: only a person's
/// approval applies it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Class {
    Green,
    Red,
}

/// Why an update is red, each naming the app it is about: `member` is the member's path,
/// `None` for the app itself.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "camelCase")]
pub enum Reason {
    /// Signed by another key than the release running now.
    #[serde(rename_all = "camelCase")]
    SignerChanged {
        member: Option<String>,
        from: String,
        to: String,
    },
    /// Destinations no destination before covers, over the whole unit.
    #[serde(rename_all = "camelCase")]
    EgressAdded { destinations: Vec<String> },
    /// Another declared state version, and no migration runs.
    #[serde(rename_all = "camelCase")]
    StateVersionChanged {
        member: Option<String>,
        from: Option<u64>,
        to: Option<u64>,
    },
    /// A member with egress joins the unit.
    #[serde(rename_all = "camelCase")]
    MemberAdded {
        member: String,
        package: String,
        egress: Vec<String>,
    },
    /// A member with egress leaves the unit.
    #[serde(rename_all = "camelCase")]
    MemberRemoved {
        member: String,
        package: String,
        egress: Vec<String>,
    },
    /// Methods of another app's `package` it now calls as the person.
    #[serde(rename_all = "camelCase")]
    DelegatedMethodsAdded {
        member: Option<String>,
        package: String,
        methods: Vec<String>,
    },
    /// A secret it declares now that it did not, or binds elsewhere: `before` is `None`
    /// for a new one; each binding as [`BundleSecret::describe`] words it.
    #[serde(rename_all = "camelCase")]
    SecretBindingChanged {
        member: Option<String>,
        name: String,
        before: Option<String>,
        after: String,
    },
    /// Not classified: `package`@`version`, which the app (or the member) runs or moves to,
    /// is no longer installed, so what it declared cannot be read. Taken as red. The node
    /// says it (a release uninstalled while another ships its WASM); [`classify`] never
    /// does.
    #[serde(rename_all = "camelCase")]
    Unclassified {
        member: Option<String>,
        package: String,
        version: String,
    },
}

/// The unit's egress before and after, and what changed: `added` are the destinations
/// after that none before covers, `removed` those before that none after covers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressDiff {
    pub before: Vec<String>,
    pub after: Vec<String>,
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

/// What [`classify`] says of an update.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Classification {
    pub class: Class,
    /// Every rule that makes it red; empty when green.
    pub reasons: Vec<Reason>,
    /// The unit's destinations before and after (empty: no data leaves Calimero).
    pub egress: EgressDiff,
    /// The app or a member that stays declares another state version.
    pub state_version_changed: bool,
    /// Methods the target ABIs declare that the running ones do not: the app's by name,
    /// a member's as `<path>.<method>`; only where both sides embed an ABI.
    pub new_methods: Vec<String>,
    /// Methods the running ABIs declare that the target ones do not, named the same way.
    pub removed_methods: Vec<String>,
    /// The app or a member that stays is signed by another key.
    pub signer_changed: bool,
}

/// Judge the update of a unit from `before` to `after` (the module's rules).
#[must_use]
pub fn classify(before: &Unit, after: &Unit) -> Classification {
    let mut reasons = Vec::new();
    let mut signer_changed = false;
    let mut state_version_changed = false;
    let mut new_methods = Vec::new();
    let mut removed_methods = Vec::new();
    let member = |path: &str| (!path.is_empty()).then(|| path.to_owned());
    let was: BTreeMap<&str, &Release> = before.parts().collect();
    let will: BTreeMap<&str, &Release> = after.parts().collect();

    for (path, to) in &will {
        let Some(from) = was.get(path).filter(|f| f.package == to.package) else {
            continue;
        };
        if from.signer != to.signer {
            signer_changed = true;
            reasons.push(Reason::SignerChanged {
                member: member(path),
                from: from.signer.clone(),
                to: to.signer.clone(),
            });
        }
        if from.state_version != to.state_version {
            state_version_changed = true;
            reasons.push(Reason::StateVersionChanged {
                member: member(path),
                from: from.state_version,
                to: to.state_version,
            });
        }
        for (package, methods) in &to.delegated {
            let had = from.delegated.get(package);
            let added: Vec<String> = methods
                .iter()
                .filter(|m| !had.is_some_and(|h| h.contains(*m)))
                .cloned()
                .collect();
            if !added.is_empty() {
                reasons.push(Reason::DelegatedMethodsAdded {
                    member: member(path),
                    package: package.clone(),
                    methods: added,
                });
            }
        }
        for (name, secret) in &to.secrets {
            let had = from.secrets.get(name);
            // Narrowed (fewer paths, the same host and place): no new reach.
            if !had.is_some_and(|h| secret.narrows(h)) {
                reasons.push(Reason::SecretBindingChanged {
                    member: member(path),
                    name: name.clone(),
                    before: had.map(|h| h.describe(name)),
                    after: secret.describe(name),
                });
            }
        }
        if let (Some(a), Some(b)) = (&from.methods, &to.methods) {
            let name = |m: &String| {
                if path.is_empty() {
                    m.clone()
                } else {
                    format!("{path}.{m}")
                }
            };
            new_methods.extend(b.difference(a).map(name));
            removed_methods.extend(a.difference(b).map(name));
        }
    }

    let stays = |path: &str, r: &Release, other: &BTreeMap<&str, &Release>| {
        other.get(path).is_some_and(|o| o.package == r.package)
    };
    for (path, r) in after.members.iter() {
        let egress = r.egress();
        if !stays(path, r, &was) && !egress.is_empty() {
            reasons.push(Reason::MemberAdded {
                member: path.clone(),
                package: r.package.clone(),
                egress: egress.into_iter().collect(),
            });
        }
    }
    for (path, r) in before.members.iter() {
        let egress = r.egress();
        if !stays(path, r, &will) && !egress.is_empty() {
            reasons.push(Reason::MemberRemoved {
                member: path.clone(),
                package: r.package.clone(),
                egress: egress.into_iter().collect(),
            });
        }
    }

    let (from, to) = (before.egress(), after.egress());
    let uncovered = |set: &BTreeSet<String>, by: &BTreeSet<String>| -> Vec<String> {
        set.iter()
            .filter(|d| !by.iter().any(|b| covers(b, d)))
            .cloned()
            .collect()
    };
    let added = uncovered(&to, &from);
    let removed = uncovered(&from, &to);
    if !added.is_empty() {
        reasons.insert(
            0,
            Reason::EgressAdded {
                destinations: added.clone(),
            },
        );
    }
    let class = if reasons.is_empty() {
        Class::Green
    } else {
        Class::Red
    };
    Classification {
        class,
        reasons,
        egress: EgressDiff {
            before: from.into_iter().collect(),
            after: to.into_iter().collect(),
            added,
            removed,
        },
        state_version_changed,
        new_methods,
        removed_methods,
        signer_changed,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const AI: &str = "https://inference.near.ai/v1/";
    const ACME: &str = "https://api.acme.com/";

    fn release(package: &str, version: &str) -> Release {
        Release {
            package: package.into(),
            version: version.into(),
            signer: "did:key:dev".into(),
            state_version: Some(1),
            methods: Some(["get".to_owned(), "set".to_owned()].into()),
            ..Release::default()
        }
    }

    fn with_outbound(mut r: Release, prefixes: &[&str]) -> Release {
        r.outbound = prefixes.iter().map(|p| (*p).to_owned()).collect();
        r
    }

    /// An app `notes` with members `store` (no egress) and `ai` (NEAR AI).
    fn unit(version: &str) -> Unit {
        Unit {
            app: release("com.acme.notes", version),
            members: BTreeMap::from([
                ("store".to_owned(), release("com.calimero.kv", "1.0.0")),
                (
                    "ai".to_owned(),
                    with_outbound(release("com.calimero.ai", "1.0.0"), &[AI]),
                ),
            ]),
        }
    }

    /// `notes` with only its `store` member: no egress.
    fn local_unit(version: &str) -> Unit {
        let mut u = unit(version);
        let _ = u.members.remove("ai");
        u
    }

    fn codes(c: &Classification) -> Vec<String> {
        c.reasons
            .iter()
            .map(|r| {
                serde_json::to_value(r).unwrap()["code"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect()
    }

    #[test]
    fn the_same_unit_again_is_green() {
        let c = classify(&unit("1.0.0"), &unit("1.1.0"));
        assert_eq!(c.class, Class::Green, "{c:?}");
        assert!(c.reasons.is_empty());
        assert_eq!(c.egress.before, vec![AI.to_owned()]);
        assert_eq!(c.egress.after, c.egress.before);
        assert!(c.egress.added.is_empty() && c.egress.removed.is_empty());
        assert!(!c.signer_changed && !c.state_version_changed);
    }

    #[test]
    fn egress_counts_every_member() {
        let u = unit("1.0.0");
        assert_eq!(u.egress(), BTreeSet::from([AI.to_owned()]));
        assert!(local_unit("1.0.0").egress().is_empty());
        assert!(Unit::of(release("p", "1.0.0")).egress().is_empty());
    }

    #[test]
    fn a_new_destination_on_the_app_is_red() {
        let mut after = local_unit("1.1.0");
        after.app = with_outbound(after.app, &[ACME]);
        let c = classify(&local_unit("1.0.0"), &after);
        assert_eq!(c.class, Class::Red);
        assert_eq!(codes(&c), ["egressAdded"]);
        assert_eq!(c.egress.added, vec![ACME.to_owned()]);
        assert!(c.egress.before.is_empty());
    }

    #[test]
    fn a_new_destination_on_a_member_is_red() {
        let mut after = unit("1.1.0");
        let ai = after.members.get_mut("ai").unwrap();
        *ai = with_outbound(ai.clone(), &[AI, "https://telemetry.acme.com/"]);
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(codes(&c), ["egressAdded"]);
        assert_eq!(
            c.egress.added,
            vec!["https://telemetry.acme.com/".to_owned()]
        );
    }

    #[test]
    fn a_destination_moved_between_members_or_narrowed_is_green() {
        // The app now calls NEAR AI itself, under a narrower prefix, and the member stops.
        let mut after = unit("1.1.0");
        after.app = with_outbound(after.app, &["https://inference.near.ai/v1/chat/"]);
        let ai = after.members.get_mut("ai").unwrap();
        *ai = with_outbound(ai.clone(), &[]);
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(c.class, Class::Green, "{c:?}");
        assert!(c.egress.added.is_empty());
        // The wider prefix went: the narrower one no longer covers all of it.
        assert_eq!(c.egress.removed, vec![AI.to_owned()]);
    }

    #[test]
    fn less_egress_is_green_and_reports_what_went() {
        let mut before = local_unit("1.0.0");
        before.app = with_outbound(before.app, &[ACME]);
        let c = classify(&before, &local_unit("1.1.0"));
        assert_eq!(c.class, Class::Green, "{c:?}");
        assert_eq!(c.egress.removed, vec![ACME.to_owned()]);
        assert!(c.egress.after.is_empty(), "no data leaves Calimero after");
        // Dropping one destination of two leaves the other.
        let mut before = unit("1.0.0");
        before.app = with_outbound(before.app, &[ACME]);
        let c = classify(&before, &unit("1.1.0"));
        assert_eq!(c.class, Class::Green, "{c:?}");
        assert_eq!(c.egress.after, vec![AI.to_owned()]);
    }

    #[test]
    fn a_wider_prefix_on_the_same_host_is_new_egress() {
        let mut before = unit("1.0.0");
        before.app = with_outbound(before.app, &["https://api.acme.com/v1/"]);
        let mut after = unit("1.1.0");
        after.app = with_outbound(after.app, &[ACME]);
        let c = classify(&before, &after);
        assert_eq!(codes(&c), ["egressAdded"]);
        assert_eq!(c.egress.added, vec![ACME.to_owned()]);
    }

    #[test]
    fn a_secret_host_no_prefix_names_is_egress() {
        let mut r = release("p", "1.0.0");
        r.secrets = BTreeMap::from([(
            "KEY".to_owned(),
            BundleSecret::header("Keys.Example.com", &["/"], "x-api-key", None),
        )]);
        assert_eq!(
            r.egress(),
            BTreeSet::from(["https://keys.example.com/".to_owned()])
        );
        // A host a prefix names adds nothing.
        let mut named = with_outbound(r.clone(), &["https://keys.example.com/v1/"]);
        named.secrets = r.secrets;
        assert_eq!(
            named.egress(),
            BTreeSet::from(["https://keys.example.com/v1/".to_owned()])
        );
    }

    /// A credential goes only where, and how, the person agreed: a new secret, or one bound
    /// to another place, is red even when the egress stays; a removed one is green.
    #[test]
    fn a_new_or_moved_secret_binding_is_red() {
        let with = |r: Release, s: &[(&str, BundleSecret)]| Release {
            secrets: s
                .iter()
                .map(|(n, b)| ((*n).to_owned(), b.clone()))
                .collect(),
            ..with_outbound(r, &[ACME])
        };
        let header = BundleSecret::header(
            "api.acme.com",
            &["/v1/"],
            "authorization",
            Some("Bearer {}"),
        );
        let body = BundleSecret::body_field("api.acme.com", &["/v1/"], "prompt");
        let before = Unit::of(with(release("p", "1.0.0"), &[("KEY", header.clone())]));

        let same = Unit::of(with(release("p", "1.1.0"), &[("KEY", header.clone())]));
        assert_eq!(classify(&before, &same).class, Class::Green);
        let gone = Unit::of(with(release("p", "1.1.0"), &[]));
        assert_eq!(classify(&before, &gone).class, Class::Green);

        let moved = Unit::of(with(release("p", "1.1.0"), &[("KEY", body)]));
        let c = classify(&before, &moved);
        assert_eq!(codes(&c), ["secretBindingChanged"]);
        assert_eq!(
            c.reasons[0],
            Reason::SecretBindingChanged {
                member: None,
                name: "KEY".to_owned(),
                before: Some(
                    "sends KEY as the Authorization header (Bearer …) to api.acme.com under /v1/"
                        .to_owned()
                ),
                after: "sends KEY as the body field \"prompt\" to api.acme.com under /v1/"
                    .to_owned(),
            }
        );
        let added = Unit::of(with(
            release("p", "1.1.0"),
            &[
                ("KEY", header.clone()),
                (
                    "OTHER",
                    BundleSecret::header("api.acme.com", &["/v1/"], "x-other", None),
                ),
            ],
        ));
        assert_eq!(codes(&classify(&before, &added)), ["secretBindingChanged"]);
        // A widened path set is red; a narrowed one is green.
        let widened = Unit::of(with(
            release("p", "1.1.0"),
            &[(
                "KEY",
                BundleSecret {
                    paths: vec!["/".to_owned()],
                    ..header.clone()
                },
            )],
        ));
        assert_eq!(
            codes(&classify(&before, &widened)),
            ["secretBindingChanged"]
        );
        let narrowed = Unit::of(with(
            release("p", "1.1.0"),
            &[(
                "KEY",
                BundleSecret {
                    paths: vec!["/v1/chat".to_owned()],
                    ..header
                },
            )],
        ));
        assert_eq!(classify(&before, &narrowed).class, Class::Green);
    }

    #[test]
    fn a_new_signer_is_red_on_the_app_or_a_member() {
        let mut after = unit("1.1.0");
        after.app.signer = "did:key:other".into();
        let c = classify(&unit("1.0.0"), &after);
        assert!(c.signer_changed);
        assert_eq!(
            c.reasons,
            [Reason::SignerChanged {
                member: None,
                from: "did:key:dev".into(),
                to: "did:key:other".into()
            }]
        );
        let mut after = unit("1.1.0");
        after.members.get_mut("store").unwrap().signer = "did:key:other".into();
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(c.class, Class::Red);
        assert!(
            matches!(&c.reasons[..], [Reason::SignerChanged { member: Some(m), .. }] if m == "store")
        );
    }

    #[test]
    fn another_state_version_is_red_and_unknown_counts_as_one() {
        let mut after = unit("1.1.0");
        after.app.state_version = Some(2);
        let c = classify(&unit("1.0.0"), &after);
        assert!(c.state_version_changed);
        assert_eq!(codes(&c), ["stateVersionChanged"]);
        let mut after = unit("1.1.0");
        after.members.get_mut("ai").unwrap().state_version = None;
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(c.class, Class::Red);
        assert!(c.state_version_changed);
    }

    #[test]
    fn a_member_with_egress_joining_or_leaving_is_red_and_one_without_is_green() {
        let mut after = unit("1.1.0");
        let _ = after.members.insert(
            "search".into(),
            with_outbound(release("com.acme.search", "1.0.0"), &[AI]),
        );
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(codes(&c), ["memberAdded"], "{c:?}");
        assert!(
            c.egress.added.is_empty(),
            "the destination was there already"
        );
        let c = classify(&after, &unit("1.2.0"));
        assert_eq!(codes(&c), ["memberRemoved"]);

        let mut after = unit("1.1.0");
        let _ = after
            .members
            .insert("cache".into(), release("com.calimero.kv", "1.0.0"));
        assert_eq!(classify(&unit("1.0.0"), &after).class, Class::Green);
        assert_eq!(classify(&after, &unit("1.0.0")).class, Class::Green);
    }

    #[test]
    fn another_package_under_the_same_name_is_a_member_leaving_and_another_joining() {
        let mut after = unit("1.1.0");
        let _ = after.members.insert(
            "ai".into(),
            with_outbound(release("com.other.ai", "1.0.0"), &[AI]),
        );
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(codes(&c), ["memberAdded", "memberRemoved"]);
        assert!(!c.signer_changed, "two packages are not a signer change");
    }

    #[test]
    fn a_new_method_called_as_the_person_is_red() {
        let mut before = unit("1.0.0");
        before.app.delegated =
            BTreeMap::from([("com.cal".to_owned(), BTreeSet::from(["read".to_owned()]))]);
        let mut after = unit("1.1.0");
        after.app.delegated = BTreeMap::from([(
            "com.cal".to_owned(),
            BTreeSet::from(["read".to_owned(), "book".to_owned()]),
        )]);
        let c = classify(&before, &after);
        assert_eq!(
            c.reasons,
            [Reason::DelegatedMethodsAdded {
                member: None,
                package: "com.cal".into(),
                methods: vec!["book".into()]
            }]
        );
        // Fewer is green.
        assert_eq!(classify(&after, &before).class, Class::Green);
    }

    #[test]
    fn new_and_removed_methods_are_reported_and_stay_green() {
        let mut after = unit("1.1.0");
        after.app.methods = Some(["get".to_owned(), "pin".to_owned()].into());
        after.members.get_mut("store").unwrap().methods =
            Some(["get".to_owned(), "set".to_owned(), "scan".to_owned()].into());
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(c.class, Class::Green);
        assert_eq!(c.new_methods, ["pin", "store.scan"]);
        assert_eq!(c.removed_methods, ["set"]);
        // Unknown on one side: not reported.
        let mut after = unit("1.1.0");
        after.app.methods = None;
        after.app.state_version = Some(1);
        let c = classify(&unit("1.0.0"), &after);
        assert!(c.new_methods.is_empty() && c.removed_methods.is_empty());
    }

    #[test]
    fn every_rule_at_once_lists_every_reason() {
        let mut after = unit("2.0.0");
        after.app = with_outbound(after.app, &[ACME]);
        after.app.signer = "did:key:other".into();
        after.app.state_version = Some(3);
        let _ = after.members.remove("ai");
        let c = classify(&unit("1.0.0"), &after);
        assert_eq!(
            codes(&c),
            [
                "egressAdded",
                "signerChanged",
                "stateVersionChanged",
                "memberRemoved"
            ]
        );
        let wire = serde_json::to_value(&c).unwrap();
        assert_eq!(wire["class"], "red");
        assert_eq!(wire["reasons"][3]["member"], "ai");
        assert_eq!(wire["egress"]["removed"], json!([AI]));
        assert!(wire.get("sealedAfter").is_none());
    }

    #[test]
    fn a_release_reads_its_facts_from_its_manifest_and_abi() {
        let manifest: BundleManifest = serde_json::from_value(json!({
            "version": "1.0", "package": "com.acme.notes", "appVersion": "1.4.0",
            "minRuntimeVersion": "0.1.0",
            "outbound": [ACME],
            "secrets": { "KEY": { "host": "api.acme.com", "paths": ["/"], "header": "x-api-key" } },
            "uses": {
                "cal": { "package": "com.cal", "range": "^1", "mode": "attach", "methods": ["book"] },
                "store": { "package": "com.calimero.kv", "range": "^1" },
            },
        }))
        .unwrap();
        let abi = json!({ "state": { "version": 4 }, "methods": { "pin": {}, "unpin": {} } });
        let r = Release::new(&manifest, "did:key:z", Some(&abi));
        assert_eq!(r.version, "1.4.0");
        assert_eq!(r.signer, "did:key:z");
        assert_eq!(r.egress(), BTreeSet::from([ACME.to_owned()]));
        assert_eq!(r.state_version, Some(4));
        assert_eq!(
            r.methods,
            Some(["pin".to_owned(), "unpin".to_owned()].into())
        );
        assert_eq!(
            r.delegated,
            BTreeMap::from([("com.cal".to_owned(), BTreeSet::from(["book".to_owned()]))])
        );
        let bare = Release::new(&manifest, "did:key:z", None);
        assert_eq!((bare.state_version, bare.methods), (None, None));
    }

    #[test]
    fn covering_follows_the_outbound_rule() {
        assert!(covers("https://a.example/", "https://a.example/v1/x"));
        assert!(covers("https://a.example/v1", "https://a.example/v1/x"));
        assert!(covers("https://a.example/v1", "https://a.example/v1"));
        assert!(!covers("https://a.example/v1", "https://a.example/v10"));
        assert!(covers("https://A.Example:443/", "https://a.example/v1/"));
        assert!(covers("https://a.example", "https://a.example/"));
        assert!(!covers("https://a.example/", "https://b.example/"));
        assert!(!covers("https://a.example/", "https://a.example:8443/"));
        assert!(!covers("https://a.example/", "https://a.example.evil/"));
        assert!(
            !covers("http://a.example/", "http://a.example/"),
            "https only"
        );
        assert!(!covers("not a url", "not a url"));
    }
}
