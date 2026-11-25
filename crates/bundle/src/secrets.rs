//! The manifest's `secrets`: credential bindings.
//!
//! A release never places, names or reads a secret in its requests. It declares, in its
//! signed manifest, each credential it needs, the endpoints it is for, and exactly where
//! the node attaches it:
//!
//! - a header: `{ "host": "api.openai.com", "paths": ["/v1/"], "header": "authorization",
//!   "format": "Bearer {}" }`, the value written into `format`'s one `{}` (absent: `{}`,
//!   the value alone);
//! - or a top-level body field, for the few APIs that take a credential there (Cloudflare
//!   Turnstile's siteverify): `{ "host": "challenges.cloudflare.com", "paths":
//!   ["/turnstile/v0/siteverify"], "bodyField": "secret" }`, into a form-urlencoded or JSON
//!   body.
//!
//! `paths` (required, 1 to [`MAX_SECRET_PATHS`]) are path prefixes on `host`, each under one
//! of the release's `outbound` prefixes; `["/"]` is the whole host, said so the person
//! approving sees it. A prefix covers a path on whole segments ([`path_covers`]):
//! `/v1/charges` covers `/v1/charges/ch_1`, never `/v1/chargesX`.
//!
//! On every request a run sends to `host` under one of `paths`, the node
//! (`calimero-outbound`) sets that header or that field itself, and refuses a request that
//! sets it, so an app cannot put the credential anywhere an upstream would read it back (a
//! prompt, an error message), nor send it to an endpoint of the host it was not bound to.
//! The binding is signed and shown before install; a new binding, or one moved or widened,
//! makes an update red (`update::Reason::SecretBindingChanged`); one narrowed to fewer
//! paths does not.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Most secrets one release declares.
pub const MAX_SECRETS: usize = 16;
/// Most bytes of a declared secret's description.
pub const MAX_SECRET_DESCRIPTION_BYTES: usize = 256;
/// Most bytes of a secret's name.
pub const MAX_SECRET_NAME_BYTES: usize = 64;
/// Most bytes of the header name or body field a secret is bound to.
pub const MAX_SECRET_FIELD_BYTES: usize = 64;
/// Most bytes of a header binding's `format`.
pub const MAX_SECRET_FORMAT_BYTES: usize = 64;
/// Most path prefixes one secret is bound to.
pub const MAX_SECRET_PATHS: usize = 8;
/// Most bytes of one of a secret's path prefixes.
pub const MAX_SECRET_PATH_BYTES: usize = 256;

/// What a header binding's `format` holds once: where the value goes.
pub const SECRET_FORMAT_SLOT: &str = "{}";

/// Headers a request may not set, and no secret may be bound to: the node sets what it
/// must itself, and these would let a request reroute itself or carry a credential the
/// operator never saw. Any `proxy-` header is refused too ([`is_forbidden_header`]).
pub const FORBIDDEN_HEADERS: &[&str] = &[
    "host",
    "connection",
    "content-length",
    "transfer-encoding",
    "te",
    "trailer",
    "keep-alive",
    "upgrade",
    "cookie",
    "proxy-authorization",
    "proxy-connection",
    "forwarded",
    "x-forwarded-for",
    "x-forwarded-host",
    "x-real-ip",
];

/// Whether a request may not set the header `lower` (lowercase): one of
/// [`FORBIDDEN_HEADERS`], or any `proxy-` one.
#[must_use]
pub fn is_forbidden_header(lower: &str) -> bool {
    FORBIDDEN_HEADERS.contains(&lower) || lower.starts_with("proxy-")
}

/// One credential a release declares (`BundleManifest::secrets`): the one host it goes to
/// and the one place the node attaches it there.
#[derive(Clone, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct BundleSecret {
    /// The host it is sent to: the host of one of the release's `outbound` prefixes,
    /// lowercase.
    pub host: String,
    /// The path prefixes on `host` it is sent to (`/v1/charges`), each under an `outbound`
    /// prefix of the release, matched on whole segments ([`path_covers`]); `["/"]` for every
    /// path. A request to `host` under none of them gets no secret.
    pub paths: Vec<String>,
    /// The header it is sent in, lowercase (`authorization`, `x-api-key`). Exclusive with
    /// `body_field`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// The header's value, with [`SECRET_FORMAT_SLOT`] once where the secret goes
    /// (`Bearer {}`). Only with `header`; absent: the value alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    /// The top-level field of a form-urlencoded or JSON body it is sent in (`secret`).
    /// Exclusive with `header`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_field: Option<String>,
    /// What it is, for whoever supplies it ("OpenAI API key").
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// The app does not work without it: a request to its host is refused while it has
    /// no value. Otherwise such a request goes out without it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub required: bool,
}

/// Where a [`BundleSecret`] is attached.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretPlacement<'a> {
    /// The header `name` (lowercase), its value `format` with the secret in its `{}`.
    Header { name: &'a str, format: &'a str },
    /// The top-level field of a form-urlencoded or JSON body.
    BodyField(&'a str),
}

impl BundleSecret {
    /// A header binding of `host`'s `paths`, as tests write one.
    #[must_use]
    pub fn header(host: &str, paths: &[&str], header: &str, format: Option<&str>) -> Self {
        Self {
            host: host.to_owned(),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            header: Some(header.to_owned()),
            format: format.map(str::to_owned),
            ..Self::default()
        }
    }

    /// A body-field binding of `host`'s `paths`.
    #[must_use]
    pub fn body_field(host: &str, paths: &[&str], field: &str) -> Self {
        Self {
            host: host.to_owned(),
            paths: paths.iter().map(|p| (*p).to_owned()).collect(),
            body_field: Some(field.to_owned()),
            ..Self::default()
        }
    }

    /// Where it is attached; `None` when it names neither a header nor a body field, or
    /// both, or a format without a header ([`BundleManifest::check_secrets`] refuses it).
    ///
    /// [`BundleManifest::check_secrets`]: crate::BundleManifest::check_secrets
    #[must_use]
    pub fn placement(&self) -> Option<SecretPlacement<'_>> {
        match (&self.header, &self.body_field, &self.format) {
            (Some(name), None, format) => Some(SecretPlacement::Header {
                name,
                format: format.as_deref().unwrap_or(SECRET_FORMAT_SLOT),
            }),
            (None, Some(field), None) => Some(SecretPlacement::BodyField(field)),
            _ => None,
        }
    }

    /// Whether it is attached to a request to `path` (on its host): one of `paths` covers it.
    #[must_use]
    pub fn applies_to(&self, path: &str) -> bool {
        self.paths.iter().any(|p| path_covers(p, path))
    }

    /// Whether `self` sends to no more than `before` does: the same host and place, and
    /// every path of `self` covered by one of `before`'s. A narrowed binding is no new
    /// reach, so it leaves an update green.
    #[must_use]
    pub fn narrows(&self, before: &Self) -> bool {
        self.host == before.host
            && self.header == before.header
            && self.format == before.format
            && self.body_field == before.body_field
            && self
                .paths
                .iter()
                .all(|p| before.paths.iter().any(|b| path_covers(b, p)))
    }

    /// Where it goes, in words: "api.openai.com, any path" or "api.stripe.com under
    /// /v1/charges, /v1/customers".
    fn destination(&self) -> String {
        if self.paths.iter().any(|p| p == "/") {
            format!("{}, any path", self.host)
        } else {
            format!("{} under {}", self.host, self.paths.join(", "))
        }
    }

    /// The binding in one line, as the consent page and `meroctl` show it: "sends NAME as
    /// the Authorization header (Bearer …) to api.openai.com under /v1/".
    #[must_use]
    pub fn describe(&self, name: &str) -> String {
        let to = self.destination();
        match self.placement() {
            Some(SecretPlacement::Header { name: h, format }) if format == SECRET_FORMAT_SLOT => {
                format!("sends {name} as the {} header to {to}", title_case(h))
            }
            Some(SecretPlacement::Header { name: h, format }) => format!(
                "sends {name} as the {} header ({}) to {to}",
                title_case(h),
                format.replace(SECRET_FORMAT_SLOT, "…"),
            ),
            Some(SecretPlacement::BodyField(f)) => {
                format!("sends {name} as the body field {f:?} to {to}")
            }
            None => format!("sends {name} to {to}"),
        }
    }
}

/// `x-api-key` -> `X-Api-Key`, how a person reads a header's name.
fn title_case(header: &str) -> String {
    header
        .split('-')
        .map(|part| {
            let mut c = part.chars();
            c.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + c.as_str()
            })
        })
        .collect::<Vec<_>>()
        .join("-")
}

/// Whether the path prefix `prefix` covers the request path `path` on whole segments: the
/// same path, or one below it at a `/` (`/v1/charges` covers `/v1/charges` and
/// `/v1/charges/ch_1`, never `/v1/chargesX`; `/v1/` covers everything under `/v1/`). The rule
/// `calimero-outbound` applies to each request, and `update::covers` to destinations.
#[must_use]
pub fn path_covers(prefix: &str, path: &str) -> bool {
    path == prefix
        || (prefix.ends_with('/') && path.starts_with(prefix))
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Whether `path` may be one of a secret's path prefixes: `/` then printable ASCII with no
/// space, `?`, `#` or `%`, no `.` or `..` segment, at most [`MAX_SECRET_PATH_BYTES`].
#[must_use]
pub fn is_secret_path(path: &str) -> bool {
    path.starts_with('/')
        && path.len() <= MAX_SECRET_PATH_BYTES
        && path
            .bytes()
            .all(|b| (0x21..=0x7e).contains(&b) && !matches!(b, b'?' | b'#' | b'%'))
        && !path.split('/').any(|seg| seg == "." || seg == "..")
}

/// Whether `name` may name a secret: `A-Z`, `0-9` and `_`, starting with a letter, at most
/// [`MAX_SECRET_NAME_BYTES`]. What an operator's binding, an owner's value and the
/// manifest call it.
#[must_use]
pub fn is_secret_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SECRET_NAME_BYTES
        && name.starts_with(|c: char| c.is_ascii_uppercase())
        && name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_')
}

/// Whether a secret may be bound to the header `name`: lowercase `a-z`, `0-9` and `-`, at
/// most [`MAX_SECRET_FIELD_BYTES`], not [`is_forbidden_header`] and not `content-type`
/// (which the node reads to place a body field).
#[must_use]
pub fn is_secret_header(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_SECRET_FIELD_BYTES
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        && !is_forbidden_header(name)
        && name != "content-type"
}

/// Whether `format` may shape a header binding's value: printable ASCII, at most
/// [`MAX_SECRET_FORMAT_BYTES`], with [`SECRET_FORMAT_SLOT`] exactly once and no other brace.
#[must_use]
pub fn is_secret_format(format: &str) -> bool {
    format.len() <= MAX_SECRET_FORMAT_BYTES
        && format.bytes().all(|b| (0x20..=0x7e).contains(&b))
        && format.matches(SECRET_FORMAT_SLOT).count() == 1
        && format.matches(['{', '}']).count() == 2
}

/// Whether a secret may be bound to the body field `field`: `A-Za-z0-9`, `_`, `.` and `-`,
/// at most [`MAX_SECRET_FIELD_BYTES`].
#[must_use]
pub fn is_secret_body_field(field: &str) -> bool {
    !field.is_empty()
        && field.len() <= MAX_SECRET_FIELD_BYTES
        && field
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// Why a manifest's `secrets` are refused.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SecretsError {
    #[error("a release declares at most {MAX_SECRETS} secrets")]
    TooMany,
    #[error("secret name {0:?} must be A-Z, 0-9 and _, starting with a letter, at most {MAX_SECRET_NAME_BYTES} bytes")]
    BadName(String),
    #[error("secret {name} names {host}, which no outbound prefix of the release covers")]
    HostNotDeclared { name: String, host: String },
    #[error("secret {0} must be bound to exactly one place: a `header` (with an optional `format`) or a `bodyField`")]
    NoPlacement(String),
    #[error("secret {name} is bound to header {header:?}: a header is lowercase a-z, 0-9 and -, at most {MAX_SECRET_FIELD_BYTES} bytes, and not one the node sets itself (host, cookie, content-type, a hop-by-hop or proxy- header)")]
    BadHeader { name: String, header: String },
    #[error("secret {0}'s format must hold {{}} exactly once, no other brace, printable ASCII, at most {MAX_SECRET_FORMAT_BYTES} bytes")]
    BadFormat(String),
    #[error("secret {name} is bound to body field {field:?}: a field is A-Z, a-z, 0-9, _, . and -, at most {MAX_SECRET_FIELD_BYTES} bytes")]
    BadBodyField { name: String, field: String },
    #[error("secret {0} must name 1 to {MAX_SECRET_PATHS} path prefixes (`paths`; [\"/\"] for every path), each `/` then printable ASCII with no space, ?, # or %, no . or .. segment, at most {MAX_SECRET_PATH_BYTES} bytes")]
    BadPaths(String),
    #[error("secret {name}'s path {path} on {host} is under no outbound prefix of the release")]
    PathNotDeclared {
        name: String,
        host: String,
        path: String,
    },
    #[error(
        "secrets {first} and {second} are bound to the same place on {host}, on paths they share"
    )]
    SamePlace {
        first: String,
        second: String,
        host: String,
    },
    #[error("secret {0}'s description is longer than {MAX_SECRET_DESCRIPTION_BYTES} bytes")]
    LongDescription(String),
}

impl SecretsError {
    /// The stable code a node answers with.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        "BAD_SECRETS"
    }
}

/// Checks each binding of `secrets` against the install rules; `outbound` are the release's
/// `outbound` prefixes, and `hosts` their hosts.
pub(crate) fn check(
    secrets: &BTreeMap<String, BundleSecret>,
    outbound: &[String],
    hosts: &std::collections::BTreeSet<String>,
) -> Result<(), SecretsError> {
    if secrets.len() > MAX_SECRETS {
        return Err(SecretsError::TooMany);
    }
    let mut places: Vec<(String, String, &BundleSecret, &String)> = Vec::new();
    for (name, secret) in secrets {
        if !is_secret_name(name) {
            return Err(SecretsError::BadName(name.clone()));
        }
        if secret.description.len() > MAX_SECRET_DESCRIPTION_BYTES {
            return Err(SecretsError::LongDescription(name.clone()));
        }
        if !hosts.contains(&secret.host) {
            return Err(SecretsError::HostNotDeclared {
                name: name.clone(),
                host: secret.host.clone(),
            });
        }
        if secret.paths.is_empty()
            || secret.paths.len() > MAX_SECRET_PATHS
            || !secret.paths.iter().all(|p| is_secret_path(p))
        {
            return Err(SecretsError::BadPaths(name.clone()));
        }
        if let Some(path) = secret.paths.iter().find(|p| {
            let url = format!("https://{}{p}", secret.host);
            !outbound.iter().any(|o| crate::update::covers(o, &url))
        }) {
            return Err(SecretsError::PathNotDeclared {
                name: name.clone(),
                host: secret.host.clone(),
                path: path.clone(),
            });
        }
        let place = match secret.placement() {
            None => return Err(SecretsError::NoPlacement(name.clone())),
            Some(SecretPlacement::Header {
                name: header,
                format,
            }) => {
                if !is_secret_header(header) {
                    return Err(SecretsError::BadHeader {
                        name: name.clone(),
                        header: header.to_owned(),
                    });
                }
                if !is_secret_format(format) {
                    return Err(SecretsError::BadFormat(name.clone()));
                }
                format!("header {header}")
            }
            Some(SecretPlacement::BodyField(field)) => {
                if !is_secret_body_field(field) {
                    return Err(SecretsError::BadBodyField {
                        name: name.clone(),
                        field: field.to_owned(),
                    });
                }
                format!("field {field}")
            }
        };
        // One place on one host takes two secrets only on paths neither shares.
        let overlaps = |other: &BundleSecret| {
            secret.paths.iter().any(|p| {
                other
                    .paths
                    .iter()
                    .any(|o| path_covers(p, o) || path_covers(o, p))
            })
        };
        if let Some((_, _, _, first)) = places
            .iter()
            .find(|(h, pl, other, _)| *h == secret.host && *pl == place && overlaps(other))
        {
            return Err(SecretsError::SamePlace {
                first: (*first).clone(),
                second: name.clone(),
                host: secret.host.clone(),
            });
        }
        places.push((secret.host.clone(), place, secret, name));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_binding_reads_as_a_person_would_say_it() {
        let openai = BundleSecret::header(
            "api.openai.com",
            &["/v1/"],
            "authorization",
            Some("Bearer {}"),
        );
        assert_eq!(
            openai.describe("OPENAI_API_KEY"),
            "sends OPENAI_API_KEY as the Authorization header (Bearer …) to api.openai.com under /v1/"
        );
        assert_eq!(
            BundleSecret::header("api.acme.com", &["/"], "x-api-key", None).describe("K"),
            "sends K as the X-Api-Key header to api.acme.com, any path"
        );
        assert_eq!(
            BundleSecret::body_field("challenges.cloudflare.com", &["/a", "/b"], "secret")
                .describe("TURNSTILE_SECRET"),
            "sends TURNSTILE_SECRET as the body field \"secret\" to challenges.cloudflare.com under /a, /b"
        );
    }

    /// A path prefix covers a path on whole segments only.
    #[test]
    fn a_path_prefix_covers_whole_segments_only() {
        for (prefix, path) in [
            ("/v1/charges", "/v1/charges"),
            ("/v1/charges", "/v1/charges/ch_1"),
            ("/v1/", "/v1/customers"),
            ("/", "/anything/at/all"),
        ] {
            assert!(path_covers(prefix, path), "{prefix} {path}");
        }
        for (prefix, path) in [
            ("/v1/charges", "/v1/chargesX"),
            ("/v1/charges", "/v1/charge"),
            ("/v1/charges", "/v1"),
            ("/v1/", "/v1"),
            ("/v1/charges", "/V1/charges"),
        ] {
            assert!(!path_covers(prefix, path), "{prefix} {path}");
        }
        assert!(is_secret_path("/"));
        assert!(is_secret_path("/v1/charges"));
        for bad in [
            "",
            "v1",
            "/v1?x",
            "/v1#x",
            "/v 1",
            "/v1/../admin",
            "/%2e%2e",
        ] {
            assert!(!is_secret_path(bad), "{bad:?}");
        }
    }

    /// Fewer paths on the same host and place is a narrowing; a wider path, another
    /// place or another host is not.
    #[test]
    fn a_binding_narrows_only_to_paths_its_earlier_one_covered() {
        let before = BundleSecret::header("api.acme.com", &["/v1/"], "x-key", None);
        let narrower = BundleSecret::header("api.acme.com", &["/v1/charges"], "x-key", None);
        assert!(narrower.narrows(&before));
        assert!(before.narrows(&before));
        assert!(!before.narrows(&narrower));
        assert!(!BundleSecret::header("api.acme.com", &["/"], "x-key", None).narrows(&before));
        assert!(!BundleSecret::header("api.acme.com", &["/v1/"], "x-other", None).narrows(&before));
    }

    #[test]
    fn a_format_holds_one_slot_and_nothing_else_to_fill() {
        assert!(is_secret_format("{}"));
        assert!(is_secret_format("Bearer {}"));
        assert!(is_secret_format("Token token={}, v=1"));
        for bad in [
            "",
            "Bearer",
            "{}{}",
            "{} {",
            "Bearer {}\r\n",
            "Bearer {x}",
            "é{}",
        ] {
            assert!(!is_secret_format(bad), "{bad:?}");
        }
        assert!(!is_secret_format(&format!(
            "{}{SECRET_FORMAT_SLOT}",
            "x".repeat(MAX_SECRET_FORMAT_BYTES - 1)
        )));
    }

    #[test]
    fn a_secret_is_never_bound_to_a_header_the_node_sets_itself() {
        assert!(is_secret_header("authorization"));
        assert!(is_secret_header("x-api-key"));
        for bad in [
            "",
            "Authorization",
            "host",
            "cookie",
            "content-type",
            "content-length",
            "proxy-authorization",
            "proxy-anything",
            "x_api_key",
            "x-api-key\r",
        ] {
            assert!(!is_secret_header(bad), "{bad:?}");
        }
    }
}
