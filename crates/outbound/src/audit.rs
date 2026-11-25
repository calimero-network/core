//! The outbound audit: what an app instance's outbound requests were, for the
//! instance's owner and managers to read.
//!
//! Every request a run hands its [`crate::OutboundHost`] is recorded here, sent or
//! refused, as one [`AuditEntry`]: when, the method, the host, the path as sent
//! (no query string, at most [`MAX_AUDIT_PATH_BYTES`]), the status or why it
//! failed ([`AuditFailure`]), the names of the credential bindings the node
//! attached to it (or the one it refused), and the
//! request's and the response's sizes. Never a secret's value, a body, a query
//! string, a header (name or value) or the whole URL.
//!
//! The audit lives in the memory of the node that ran the request, one ring per
//! instance: at most [`MAX_AUDIT_ENTRIES`] entries, none older than
//! [`AUDIT_RETENTION_MS`], for at most [`MAX_AUDIT_INSTANCES`] instances. It is
//! not replicated and does not survive a restart: the node's API merges the
//! rings of the instance's replicas when its owner reads it. It is never logged
//! and never a metric.

use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use calimero_primitives::outbound::{OutboundMethod, OutboundRefusal, OutboundRequest, MAX_URL};
use serde::{Deserialize, Serialize};

/// The most entries one node keeps for one instance; the oldest goes first.
pub const MAX_AUDIT_ENTRIES: usize = 100;

/// How long one node keeps an entry: 7 days, in milliseconds.
pub const AUDIT_RETENTION_MS: u64 = 7 * 24 * 60 * 60 * 1000;

/// The most instances one node keeps an audit for; past it, the instance whose
/// last request is the oldest is forgotten.
pub const MAX_AUDIT_INSTANCES: usize = 1024;

/// The longest path an entry keeps, in bytes; a longer one is cut at a character
/// boundary and marked `pathTruncated`.
pub const MAX_AUDIT_PATH_BYTES: usize = 128;

/// The most secret names one entry keeps (a release binds at most `MAX_SECRETS`).
pub const MAX_AUDIT_SECRET_NAMES: usize = 16;

/// Why a request was not answered by its host.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum AuditFailure {
    /// The run's request could not be read.
    Malformed,
    /// A gate refused it: the operator's hosts, the release's declared prefixes,
    /// a size limit, a header, the scheme or the port.
    Refused,
    /// Its prefix is not approved for the app's publisher ([`OutboundRefusal::NotApproved`]).
    NotApproved,
    /// A later release declares its prefix and nobody was asked yet
    /// ([`OutboundRefusal::AwaitingApproval`]).
    AwaitingApproval,
    /// The instance's organization, or the instance, turned egress off
    /// ([`OutboundRefusal::EgressOff`]): refused before any other gate.
    EgressOff,
    /// A credential could not be attached: a required binding with no value, the
    /// request setting a bound header or field itself, an operator's value bound to
    /// other hosts, a value that does not fit its place, or a retired
    /// `{{secret:NAME}}` placeholder.
    Secret,
    /// It was sent, and the connection, the timeout or the response's size failed it.
    Failed,
}

/// One outbound request of an instance, as its owner may see it.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuditEntry {
    /// When the node handled it, in Unix milliseconds of the node's clock.
    pub at_ms: u64,
    /// `GET`, `POST`, `PUT`, `PATCH` or `DELETE`.
    pub method: String,
    /// The URL's host, lower case; empty when the URL could not be read.
    pub host: String,
    /// The URL's path, with no query string or fragment, at most
    /// [`MAX_AUDIT_PATH_BYTES`]; empty when the URL could not be read.
    pub path: String,
    /// The path was longer and was cut.
    pub path_truncated: bool,
    /// The host's HTTP status, when it answered (absent otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<u16>,
    /// Why it was not answered, when it was not (absent otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<AuditFailure>,
    /// The names of the credential bindings the node attached to it, sorted; for a
    /// `secret` refusal, the binding refused (when one was).
    pub secrets: Vec<String>,
    /// The request body's size as the run gave it, before any credential was attached.
    pub request_bytes: u64,
    /// The response body's size as the run got it, after redaction; 0 when none.
    pub response_bytes: u64,
}

impl AuditEntry {
    /// An entry for `request`, at `at_ms`, with no outcome yet.
    #[must_use]
    pub fn of(request: &OutboundRequest, at_ms: u64) -> Self {
        let (host, path, path_truncated) = host_and_path(&request.url);
        Self {
            at_ms,
            method: method_name(request.method).to_owned(),
            host,
            path,
            path_truncated,
            status: None,
            failure: None,
            secrets: Vec::new(),
            request_bytes: request.body.len() as u64,
            response_bytes: 0,
        }
    }

    /// A request the run's bytes did not decode into: nothing about it is known.
    #[must_use]
    pub fn malformed(at_ms: u64) -> Self {
        Self {
            at_ms,
            method: String::new(),
            host: String::new(),
            path: String::new(),
            path_truncated: false,
            status: None,
            failure: Some(AuditFailure::Malformed),
            secrets: Vec::new(),
            request_bytes: 0,
            response_bytes: 0,
        }
    }
}

impl AuditEntry {
    /// A connector call: `connector`'s host and `operation`'s declared path (its
    /// `{field}` slots as declared, never the ids the input filled in), or what of
    /// them the call named (empty for a connector or operation the release does not
    /// declare).
    pub(crate) fn of_call(
        connector: Option<&crate::Connector>,
        operation: &str,
        request_bytes: usize,
        at_ms: u64,
    ) -> Self {
        let op = connector.and_then(|c| c.operations.iter().find(|o| o.name == operation));
        let mut path = op.map_or_else(String::new, |o| o.path.to_owned());
        let path_truncated = path.len() > MAX_AUDIT_PATH_BYTES;
        if path_truncated {
            let mut cut = MAX_AUDIT_PATH_BYTES;
            while !path.is_char_boundary(cut) {
                cut -= 1;
            }
            path.truncate(cut);
        }
        Self {
            at_ms,
            method: op.map_or("", |o| method_name(o.method)).to_owned(),
            host: connector.map_or_else(String::new, |c| c.host.to_owned()),
            path,
            path_truncated,
            status: None,
            failure: None,
            secrets: Vec::new(),
            request_bytes: request_bytes as u64,
            response_bytes: 0,
        }
    }
}

/// What an entry keeps of the binding names the node attached: sorted, each once,
/// at most [`MAX_AUDIT_SECRET_NAMES`]. Never a value: the names are the manifest's.
#[must_use]
pub fn secret_names(attached: &[String]) -> Vec<String> {
    let mut names = attached.to_vec();
    names.sort();
    names.dedup();
    names.truncate(MAX_AUDIT_SECRET_NAMES);
    names
}

/// The host and path of `url`, with no query string, fragment or credentials, the
/// path cut at [`MAX_AUDIT_PATH_BYTES`]; empty for a URL that does not parse.
///
/// The path is kept as the app sent it, id-like segments included: what an
/// owner needs is which endpoint was called (`/v1/refunds` rather than
/// `/v1/charges`), which a rule guessing at ids would blur, and the readers are
/// the instance's owner and managers, who already read the instance's data.
fn host_and_path(url: &str) -> (String, String, bool) {
    if url.len() > MAX_URL {
        return (String::new(), String::new(), false);
    }
    let Ok(url) = url::Url::parse(url) else {
        return (String::new(), String::new(), false);
    };
    let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
    let path = url.path();
    if path.len() <= MAX_AUDIT_PATH_BYTES {
        return (host, path.to_owned(), false);
    }
    let mut cut = MAX_AUDIT_PATH_BYTES;
    while !path.is_char_boundary(cut) {
        cut -= 1;
    }
    (host, path[..cut].to_owned(), true)
}

const fn method_name(method: OutboundMethod) -> &'static str {
    match method {
        OutboundMethod::Get => "GET",
        OutboundMethod::Post => "POST",
        OutboundMethod::Put => "PUT",
        OutboundMethod::Patch => "PATCH",
        OutboundMethod::Delete => "DELETE",
    }
}

/// The binding a refusal `prepare` returned names, if it refused one.
pub(crate) fn refused_binding(reason: &eyre::Report) -> Vec<String> {
    reason
        .downcast_ref::<crate::SecretRefused>()
        .and_then(|r| r.binding.clone())
        .into_iter()
        .collect()
}

/// The failure kind of a refusal `prepare` returned.
pub(crate) fn refusal_kind(reason: &eyre::Report) -> AuditFailure {
    if reason.downcast_ref::<crate::SecretRefused>().is_some() {
        return AuditFailure::Secret;
    }
    match OutboundRefusal::of(&reason.to_string()) {
        Some(OutboundRefusal::NotApproved) => AuditFailure::NotApproved,
        Some(OutboundRefusal::AwaitingApproval) => AuditFailure::AwaitingApproval,
        Some(OutboundRefusal::EgressOff) => AuditFailure::EgressOff,
        None => AuditFailure::Refused,
    }
}

/// Now, in Unix milliseconds.
pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

/// One node's audit rings, by instance.
#[derive(Debug, Default)]
pub struct OutboundAudit {
    rings: Mutex<HashMap<String, VecDeque<AuditEntry>>>,
}

impl OutboundAudit {
    /// Records `entry` for `instance`, dropping what retention no longer keeps.
    pub fn record(&self, instance: &str, entry: AuditEntry) {
        let now = entry.at_ms;
        let Ok(mut rings) = self.rings.lock() else {
            return;
        };
        if !rings.contains_key(instance) && rings.len() >= MAX_AUDIT_INSTANCES {
            rings.retain(|_, ring| {
                expire(ring, now);
                !ring.is_empty()
            });
            if rings.len() >= MAX_AUDIT_INSTANCES {
                let stalest = rings
                    .iter()
                    .min_by_key(|(_, ring)| ring.back().map_or(0, |e| e.at_ms))
                    .map(|(id, _)| id.clone());
                if let Some(id) = stalest {
                    let _gone = rings.remove(&id);
                }
            }
        }
        let ring = rings.entry(instance.to_owned()).or_default();
        expire(ring, now);
        while ring.len() >= MAX_AUDIT_ENTRIES {
            let _oldest = ring.pop_front();
        }
        ring.push_back(entry);
    }

    /// `instance`'s entries this node still keeps at `now_ms`, oldest first.
    #[must_use]
    pub fn entries_at(&self, instance: &str, now_ms: u64) -> Vec<AuditEntry> {
        let Ok(mut rings) = self.rings.lock() else {
            return Vec::new();
        };
        let Some(ring) = rings.get_mut(instance) else {
            return Vec::new();
        };
        expire(ring, now_ms);
        ring.iter().cloned().collect()
    }

    /// `instance`'s entries this node still keeps, oldest first.
    #[must_use]
    pub fn entries(&self, instance: &str) -> Vec<AuditEntry> {
        self.entries_at(instance, now_ms())
    }
}

fn expire(ring: &mut VecDeque<AuditEntry>, now_ms: u64) {
    let oldest = now_ms.saturating_sub(AUDIT_RETENTION_MS);
    while ring.front().is_some_and(|e| e.at_ms < oldest) {
        let _old = ring.pop_front();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn get(url: &str) -> OutboundRequest {
        OutboundRequest {
            method: OutboundMethod::Get,
            url: url.to_owned(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    #[test]
    fn the_path_drops_the_query_and_is_cut_at_its_bound() {
        let e = AuditEntry::of(
            &get("https://API.example.com/v1/customers/cus_123?email=a%40b.c&token=t#frag"),
            1,
        );
        assert_eq!(e.host, "api.example.com");
        assert_eq!(e.path, "/v1/customers/cus_123");
        assert!(!e.path_truncated);

        let long = format!("https://api.example.com/{}", "é".repeat(200));
        let e = AuditEntry::of(&get(&long), 1);
        assert!(e.path_truncated);
        assert!(e.path.len() <= MAX_AUDIT_PATH_BYTES, "{}", e.path.len());
        assert!(e.path.starts_with("/%C3%A9"), "{}", e.path);

        let e = AuditEntry::of(&get("not a url"), 1);
        assert_eq!((e.host.as_str(), e.path.as_str()), ("", ""));
    }

    #[test]
    fn secret_names_are_sorted_once_and_bounded() {
        let attached: Vec<String> = ["STRIPE_KEY", "A_KEY", "STRIPE_KEY"]
            .iter()
            .map(|n| (*n).to_owned())
            .chain((0..40).map(|i| format!("K{i:02}")))
            .collect();
        let names = secret_names(&attached);
        assert_eq!(names.len(), MAX_AUDIT_SECRET_NAMES);
        assert_eq!(names[..2], ["A_KEY", "K00"]);
        assert_eq!(
            names.iter().filter(|n| *n == "STRIPE_KEY").count(),
            0,
            "cut past the bound"
        );
        assert_eq!(secret_names(&attached[..3]), ["A_KEY", "STRIPE_KEY"]);
    }

    #[test]
    fn a_ring_keeps_its_bound_and_its_age() {
        let audit = OutboundAudit::default();
        let request = get("https://api.example.com/v1/x");
        for i in 0..(MAX_AUDIT_ENTRIES as u64 + 10) {
            audit.record("app", AuditEntry::of(&request, 1_000 + i));
        }
        let kept = audit.entries_at("app", 2_000);
        assert_eq!(kept.len(), MAX_AUDIT_ENTRIES);
        assert_eq!(kept[0].at_ms, 1_010, "the oldest went first");

        let later = 1_000 + MAX_AUDIT_ENTRIES as u64 + 9 + AUDIT_RETENTION_MS;
        assert_eq!(
            audit.entries_at("app", later).len(),
            1,
            "only the newest is young enough"
        );
        assert!(audit.entries_at("app", later + 1).is_empty());
        assert!(audit.entries_at("other", later).is_empty());
    }

    #[test]
    fn past_its_instance_bound_the_stalest_instance_is_forgotten() {
        let audit = OutboundAudit::default();
        let request = get("https://api.example.com/v1/x");
        for i in 0..MAX_AUDIT_INSTANCES as u64 {
            audit.record(&format!("app{i}"), AuditEntry::of(&request, 10 + i));
        }
        audit.record("new", AuditEntry::of(&request, 10_000));
        assert!(audit.entries_at("app0", 10_000).is_empty());
        assert_eq!(audit.entries_at("app1", 10_000).len(), 1);
        assert_eq!(audit.entries_at("new", 10_000).len(), 1);
        assert_eq!(audit.rings.lock().unwrap().len(), MAX_AUDIT_INSTANCES);
    }
}
