//! What changed between two ABIs of an app, and whether its callers notice.
//!
//! Parameters are what callers send, so a new parameter type must accept every value the old
//! one did (it may widen); results, errors and events are what callers receive, so a new type
//! must stay within the old one (it may narrow). The state section is checked as an upgrade
//! reads it: a removed field or a field whose collection kind changed leaves data the new code
//! cannot read, which only a state version bump (and its migration) may do.

use std::collections::BTreeMap;
use std::fmt;

use serde_json::{json, Value};

use crate::abi::{AppAbi, Fields, Method};
use crate::ty::{Int, Type};

/// Whether a change breaks existing callers.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Compatible,
    Breaking,
}

/// One change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    pub severity: Severity,
    /// What changed: `method book`, `error NotFound`, `event Booked`, `state`, `abi`.
    pub subject: String,
    pub message: String,
}

/// Every change between two ABIs.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    pub changes: Vec<Change>,
    /// Set when the state changed in a way the old data cannot be read and the state version
    /// did not move: publishing it would strand the app's data, so the publish path refuses it.
    pub state_error: Option<String>,
}

impl Report {
    #[must_use]
    pub fn is_breaking(&self) -> bool {
        self.changes
            .iter()
            .any(|c| c.severity == Severity::Breaking)
    }

    /// The report as JSON: `{ breaking, stateError, changes: [{ severity, subject, message }] }`.
    #[must_use]
    pub fn to_json(&self) -> Value {
        json!({
            "breaking": self.is_breaking(),
            "stateError": self.state_error,
            "changes": self.changes.iter().map(|c| json!({
                "severity": match c.severity {
                    Severity::Breaking => "breaking",
                    Severity::Compatible => "compatible",
                },
                "subject": c.subject,
                "message": c.message,
            })).collect::<Vec<_>>(),
        })
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.changes.is_empty() {
            return writeln!(f, "no changes");
        }
        let mut changes: Vec<&Change> = self.changes.iter().collect();
        changes.sort_by_key(|c| std::cmp::Reverse(c.severity));
        for c in changes {
            let tag = match c.severity {
                Severity::Breaking => "breaking  ",
                Severity::Compatible => "compatible",
            };
            writeln!(f, "{tag} {}: {}", c.subject, c.message)?;
        }
        if let Some(error) = &self.state_error {
            writeln!(f, "error      state: {error}")?;
        }
        Ok(())
    }
}

/// The changes from `old` to `new`.
#[must_use]
pub fn diff(old: &AppAbi, new: &AppAbi) -> Report {
    let mut r = Diff::default();
    if old.abi_version != new.abi_version {
        r.push(
            Severity::Breaking,
            "abi",
            format!(
                "abiVersion {} -> {}: a node reads only the versions it supports",
                old.abi_version, new.abi_version
            ),
        );
    }
    if old.world != new.world {
        let minor = |w: &Option<String>| {
            w.as_deref().map(|w| {
                let (pkg, ver) = w.split_once('@').unwrap_or((w, ""));
                let mm: Vec<&str> = ver.split('.').take(2).collect();
                format!("{pkg}@{}", mm.join("."))
            })
        };
        let severity = if minor(&old.world) == minor(&new.world) {
            Severity::Compatible
        } else {
            Severity::Breaking
        };
        r.push(
            severity,
            "abi",
            format!(
                "world {} -> {}",
                old.world.as_deref().unwrap_or("none"),
                new.world.as_deref().unwrap_or("none")
            ),
        );
    }
    r.methods(old, new);
    r.payloads("error", &old.errors, &new.errors);
    r.payloads("event", &old.events, &new.events);
    r.state(old, new);
    Report {
        changes: r.changes,
        state_error: r.state_error,
    }
}

#[derive(Default)]
struct Diff {
    changes: Vec<Change>,
    state_error: Option<String>,
}

/// Whether others than the app itself call the method: clients, the node, other apps.
fn called_from_outside(m: &Method) -> bool {
    m.client_callable() || m.allow_system || !m.allow_apps.is_empty()
}

fn same_signature(a: &Method, b: &Method) -> bool {
    a.kind == b.kind && a.params == b.params && a.returns == b.returns
}

impl Diff {
    fn push(&mut self, severity: Severity, subject: impl Into<String>, message: impl Into<String>) {
        self.changes.push(Change {
            severity,
            subject: subject.into(),
            message: message.into(),
        });
    }

    fn methods(&mut self, old: &AppAbi, new: &AppAbi) {
        for (name, o) in &old.methods {
            let subject = format!("method {name}");
            let Some(n) = new.methods.get(name) else {
                let renamed: Vec<&str> = new
                    .methods
                    .iter()
                    .filter(|(k, m)| !old.methods.contains_key(*k) && same_signature(o, m))
                    .map(|(k, _)| k.as_str())
                    .collect();
                let severity = if called_from_outside(o) {
                    Severity::Breaking
                } else {
                    Severity::Compatible
                };
                let hint = if renamed.is_empty() {
                    String::new()
                } else {
                    format!(" (renamed to {}?)", renamed.join(" or "))
                };
                self.push(severity, subject, format!("removed{hint}"));
                continue;
            };
            self.method(&subject, o, n);
        }
        for (name, n) in &new.methods {
            if !old.methods.contains_key(name) {
                self.push(
                    Severity::Compatible,
                    format!("method {name}"),
                    format!("added ({})", n.kind.name()),
                );
            }
        }
    }

    fn method(&mut self, subject: &str, o: &Method, n: &Method) {
        if o.kind != n.kind {
            self.push(
                Severity::Breaking,
                subject,
                format!("kind {} -> {}", o.kind.name(), n.kind.name()),
            );
            return;
        }
        if !called_from_outside(o) {
            if !same_signature(o, n) {
                self.push(
                    Severity::Compatible,
                    subject,
                    "signature changed (no client, node or app calls it)",
                );
            }
            if o.every != n.every {
                self.push(
                    Severity::Compatible,
                    subject,
                    format!(
                        "every {} -> {}",
                        o.every.as_deref().unwrap_or("-"),
                        n.every.as_deref().unwrap_or("-")
                    ),
                );
            }
            if o.compute != n.compute {
                self.push(Severity::Compatible, subject, "compute caps changed");
            }
            return;
        }
        self.params(subject, o, n);
        if o.returns != n.returns {
            let (severity, how) = if o.returns == Type::Unit {
                (Severity::Compatible, "now returns a value")
            } else if widens(&n.returns, &o.returns) {
                (Severity::Compatible, "narrowed")
            } else {
                (Severity::Breaking, "changed")
            };
            self.push(
                severity,
                subject,
                format!("result {how}: {} -> {}", o.returns, n.returns),
            );
        }
        self.flags(subject, o, n);
    }

    fn params(&mut self, subject: &str, o: &Method, n: &Method) {
        let old: BTreeMap<&str, &Type> = o.params.iter().map(|(k, t)| (k.as_str(), t)).collect();
        let new: BTreeMap<&str, &Type> = n.params.iter().map(|(k, t)| (k.as_str(), t)).collect();
        for (p, ot) in &old {
            match new.get(p) {
                None => self.push(
                    Severity::Breaking,
                    subject,
                    format!("param `{p}` removed: callers that pass it are no longer heard"),
                ),
                Some(nt) if ot == nt => {}
                Some(nt) if widens(ot, nt) => self.push(
                    Severity::Compatible,
                    subject,
                    format!("param `{p}` widened: {ot} -> {nt}"),
                ),
                Some(nt) => self.push(
                    Severity::Breaking,
                    subject,
                    format!("param `{p}` changed: {ot} -> {nt}"),
                ),
            }
        }
        for (p, nt) in &new {
            if old.contains_key(p) {
                continue;
            }
            if nt.is_optional() {
                self.push(
                    Severity::Compatible,
                    subject,
                    format!("optional param `{p}` added ({nt})"),
                );
            } else {
                self.push(
                    Severity::Breaking,
                    subject,
                    format!("required param `{p}` added ({nt}): callers that leave it out fail"),
                );
            }
        }
        let order = |m: &Method| m.params.iter().map(|(k, _)| k.clone()).collect::<Vec<_>>();
        if old.keys().eq(new.keys()) && order(o) != order(n) {
            self.push(
                Severity::Compatible,
                subject,
                "params reordered (JSON passes them by name)",
            );
        }
    }

    fn flags(&mut self, subject: &str, o: &Method, n: &Method) {
        let mut flag = |was: bool, now: bool, on: (Severity, &str), off: (Severity, &str)| {
            if was == now {
                return;
            }
            let (severity, message) = if now { on } else { off };
            self.push(severity, subject, message);
        };
        flag(
            o.confirm,
            n.confirm,
            (
                Severity::Breaking,
                "now needs a passkey (confirm): callers get 412 CONFIRM_REQUIRED",
            ),
            (Severity::Compatible, "no longer needs a passkey"),
        );
        flag(
            o.public,
            n.public,
            (Severity::Compatible, "now public"),
            (
                Severity::Breaking,
                "no longer public: callers who are not signed in are refused",
            ),
        );
        flag(
            o.allow_system,
            n.allow_system,
            (Severity::Compatible, "now accepts the node (allowSystem)"),
            (
                Severity::Breaking,
                "no longer accepts the node (allowSystem)",
            ),
        );
        flag(
            o.allow_action,
            n.allow_action,
            (
                Severity::Breaking,
                "now allow(action) only: clients can no longer call it",
            ),
            (Severity::Compatible, "no longer allow(action) only"),
        );
        let lost: Vec<&String> = if n.allow_apps.iter().any(|a| a == "*") {
            Vec::new()
        } else {
            o.allow_apps
                .iter()
                .filter(|a| !n.allow_apps.contains(a))
                .collect()
        };
        if !lost.is_empty() {
            self.push(
                Severity::Breaking,
                subject,
                format!(
                    "no longer accepts xcalls from {}",
                    lost.iter()
                        .map(|a| a.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            );
        }
        if o.rate != n.rate {
            self.push(
                Severity::Compatible,
                subject,
                format!(
                    "rate {} -> {}",
                    o.rate.as_deref().unwrap_or("none"),
                    n.rate.as_deref().unwrap_or("none")
                ),
            );
        }
    }

    /// Errors and events: what callers receive.
    fn payloads(
        &mut self,
        what: &str,
        old: &BTreeMap<String, Fields>,
        new: &BTreeMap<String, Fields>,
    ) {
        for (name, of) in old {
            let subject = format!("{what} {name}");
            let Some(nf) = new.get(name) else {
                self.push(
                    Severity::Breaking,
                    subject,
                    "removed: code that handles it, and clients generated with it, break",
                );
                continue;
            };
            for (field, ot) in of {
                match nf.get(field) {
                    None => self.push(
                        Severity::Breaking,
                        &subject,
                        format!("field `{field}` removed"),
                    ),
                    Some(nt) if nt == ot => {}
                    Some(nt) if widens(nt, ot) => self.push(
                        Severity::Compatible,
                        &subject,
                        format!("field `{field}` narrowed: {ot} -> {nt}"),
                    ),
                    Some(nt) => self.push(
                        Severity::Breaking,
                        &subject,
                        format!("field `{field}` changed: {ot} -> {nt}"),
                    ),
                }
            }
            for (field, nt) in nf {
                if !of.contains_key(field) {
                    self.push(
                        Severity::Compatible,
                        &subject,
                        format!("field `{field}` added ({nt})"),
                    );
                }
            }
        }
        for name in new.keys().filter(|k| !old.contains_key(*k)) {
            self.push(Severity::Compatible, format!("{what} {name}"), "added");
        }
    }

    fn state(&mut self, old: &AppAbi, new: &AppAbi) {
        let (os, ns) = (&old.state, &new.state);
        let mut unreadable = Vec::new();
        for (field, kind) in &os.fields {
            match ns.fields.get(field) {
                None => unreadable.push(format!(
                    "field `{field}` removed (its data stays under the old name)"
                )),
                Some(k) if k != kind => {
                    unreadable.push(format!("field `{field}` changed from {kind} to {k}"));
                }
                Some(_) => {}
            }
        }
        for (field, kind) in &ns.fields {
            if !os.fields.contains_key(field) {
                self.push(
                    Severity::Compatible,
                    "state",
                    format!("field `{field}` added ({kind})"),
                );
            }
        }
        if os.search != ns.search {
            self.push(
                Severity::Compatible,
                "state",
                "search fields changed: the index is rebuilt",
            );
        }
        let (Some(ov), Some(nv)) = (os.version, ns.version) else {
            for u in unreadable {
                self.push(Severity::Breaking, "state", u);
            }
            return;
        };
        match nv.cmp(&ov) {
            std::cmp::Ordering::Less => {
                self.push(
                    Severity::Breaking,
                    "state",
                    format!("version {ov} -> {nv}: the node refuses a downgrade"),
                );
            }
            std::cmp::Ordering::Greater if nv > ov + 1 => {
                self.push(
                    Severity::Breaking,
                    "state",
                    format!("version {ov} -> {nv}: the node upgrades one state version at a time"),
                );
            }
            std::cmp::Ordering::Greater => self.push(
                Severity::Compatible,
                "state",
                format!(
                    "version {ov} -> {nv}: the upgrade runs the `#[app::migrate]` edge from {ov}"
                ),
            ),
            std::cmp::Ordering::Equal => {}
        }
        if unreadable.is_empty() {
            return;
        }
        if nv == ov {
            self.state_error = Some(format!(
                "{} while the state version stays {ov}: bump `#[app::state(version = {})]` and migrate the data",
                unreadable.join("; "),
                ov + 1
            ));
        }
        for u in unreadable {
            self.push(Severity::Breaking, "state", u);
        }
    }
}

/// Whether every JSON value of `from` is also a value of `to`.
#[must_use]
pub fn widens(from: &Type, to: &Type) -> bool {
    if from == to {
        return true;
    }
    match (from, to) {
        (Type::Option(f), Type::Option(t)) => widens(f, t),
        (f, Type::Option(t)) => widens(f, t),
        (Type::Int(f), Type::Int(t)) => int_widens(*f, *t),
        (Type::Int(f), Type::Float(true)) => {
            let (lo, hi) = f.range();
            !f.is_string() && lo >= -(1 << 53) && hi <= 1 << 53
        }
        (Type::Float(false), Type::Float(true)) | (Type::Char, Type::String) => true,
        (Type::List(f), Type::List(t)) | (Type::Set(f), Type::List(t)) => widens(f, t),
        (Type::Set(f), Type::Set(t)) => widens(f, t),
        (Type::Array(f, _), Type::List(t)) => widens(f, t),
        (Type::Array(f, n), Type::Array(t, m)) => n == m && widens(f, t),
        (Type::Tuple(f), Type::Tuple(t)) => {
            f.len() == t.len() && f.iter().zip(t).all(|(f, t)| widens(f, t))
        }
        (Type::Map(fk, fv), Type::Map(tk, tv)) => widens(fk, tk) && widens(fv, tv),
        _ => false,
    }
}

/// An integer widens to another of the same JSON form (number or decimal string) whose range
/// holds its own.
fn int_widens(from: Int, to: Int) -> bool {
    if from.is_string() != to.is_string() {
        return false;
    }
    let ((flo, fhi), (tlo, thi)) = (from.range(), to.range());
    tlo <= flo && fhi <= thi
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn abi(methods: Value) -> AppAbi {
        abi_with(
            methods,
            json!({}),
            json!({}),
            json!({ "fields": {}, "version": 1 }),
        )
    }

    fn abi_with(methods: Value, errors: Value, events: Value, state: Value) -> AppAbi {
        AppAbi::from_json(&json!({
            "abiVersion": 1,
            "world": "calimero:app@0.5.1",
            "methods": methods,
            "errors": errors,
            "events": events,
            "state": state,
        }))
        .unwrap()
    }

    fn call(params: Value, returns: Value) -> Value {
        json!({ "kind": "call", "params": params, "returns": returns })
    }

    fn breaking(r: &Report) -> Vec<String> {
        r.changes
            .iter()
            .filter(|c| c.severity == Severity::Breaking)
            .map(|c| format!("{}: {}", c.subject, c.message))
            .collect()
    }

    #[test]
    fn the_same_abi_has_no_changes() {
        let a = abi(json!({ "set": call(json!([["key", "string"]]), Value::Null) }));
        let r = diff(&a, &a);
        assert!(r.changes.is_empty());
        assert_eq!(r.to_string(), "no changes\n");
    }

    #[test]
    fn a_removed_method_breaks_and_a_rename_is_named() {
        let old = abi(json!({ "set": call(json!([["key", "string"]]), Value::Null) }));
        let new = abi(json!({ "put": call(json!([["key", "string"]]), Value::Null) }));
        let r = diff(&old, &new);
        assert_eq!(breaking(&r), ["method set: removed (renamed to put?)"]);
        assert!(r.is_breaking());
    }

    #[test]
    fn a_timer_removed_is_compatible() {
        let old = abi(json!({ "tick": { "kind": "timer", "params": [], "every": "30s" } }));
        let new = abi(json!({}));
        assert!(!diff(&old, &new).is_breaking());
    }

    #[test]
    fn params() {
        let old = abi(
            json!({ "m": call(json!([["a", "u32"], ["b", "string"], ["c", "u32"]]), Value::Null) }),
        );
        let new = abi(json!({ "m": call(json!([
            ["a", "u64"],            // number -> decimal string
            ["b", "option<string>"], // widened
            ["d", "string"],         // required, new
            ["e", "option<u32>"],    // optional, new
        ]), Value::Null) }));
        let r = diff(&old, &new);
        assert_eq!(
            breaking(&r),
            [
                "method m: param `a` changed: u32 -> u64",
                "method m: param `c` removed: callers that pass it are no longer heard",
                "method m: required param `d` added (string): callers that leave it out fail",
            ]
        );
        let all = r.to_string();
        assert!(all.contains("compatible method m: param `b` widened: string -> option<string>"));
        assert!(all.contains("compatible method m: optional param `e` added (option<u32>)"));
    }

    #[test]
    fn integers_widen_within_their_json_form() {
        let t = |s: &str| Type::parse(s);
        assert!(widens(&t("u8"), &t("u32")));
        assert!(!widens(&t("u32"), &t("i64")));
        assert!(widens(&t("i32"), &t("f64")));
        assert!(!widens(&t("u64"), &t("i64")));
        assert!(widens(&t("i64"), &t("option<i64>")));
        assert!(!widens(&t("u32"), &t("u16")));
        assert!(widens(&t("list<u16>"), &t("list<u32>")));
        assert!(widens(&t("char"), &t("string")));
    }

    #[test]
    fn results_may_narrow_not_widen() {
        let old = abi(json!({
            "a": call(json!([]), json!("option<string>")),
            "b": call(json!([]), json!("string")),
            "c": call(json!([]), Value::Null),
            "d": call(json!([]), json!("u16")),
        }));
        let new = abi(json!({
            "a": call(json!([]), json!("string")),
            "b": call(json!([]), json!("option<string>")),
            "c": call(json!([]), json!("bool")),
            "d": call(json!([]), json!("u32")),
        }));
        assert_eq!(
            breaking(&diff(&old, &new)),
            [
                "method b: result changed: string -> option<string>",
                "method d: result changed: u16 -> u32",
            ]
        );
    }

    #[test]
    fn kinds_and_flags() {
        let old = abi(json!({
            "a": { "kind": "call", "params": [] },
            "b": { "kind": "call", "params": [] },
            "c": { "kind": "view", "params": [], "public": true },
            "d": { "kind": "call", "params": [], "allowApps": ["x", "y"] },
            "e": { "kind": "call", "params": [], "confirm": true },
        }));
        let new = abi(json!({
            "a": { "kind": "view", "params": [] },
            "b": { "kind": "call", "params": [], "confirm": true },
            "c": { "kind": "view", "params": [] },
            "d": { "kind": "call", "params": [], "allowApps": ["x"] },
            "e": { "kind": "call", "params": [] },
        }));
        assert_eq!(
            breaking(&diff(&old, &new)),
            [
                "method a: kind call -> view",
                "method b: now needs a passkey (confirm): callers get 412 CONFIRM_REQUIRED",
                "method c: no longer public: callers who are not signed in are refused",
                "method d: no longer accepts xcalls from y",
            ]
        );
    }

    #[test]
    fn errors_and_events() {
        let old = abi_with(
            json!({}),
            json!({ "NotFound": { "key": "string" }, "Gone": {} }),
            json!({ "Set": { "key": "string", "at": "u64" } }),
            json!({ "fields": {}, "version": 1 }),
        );
        let new = abi_with(
            json!({}),
            json!({ "NotFound": { "key": "string", "hint": "string" }, "Taken": {} }),
            json!({ "Set": { "key": "string", "at": "u64" } }),
            json!({ "fields": {}, "version": 1 }),
        );
        let r = diff(&old, &new);
        assert_eq!(
            breaking(&r),
            ["error Gone: removed: code that handles it, and clients generated with it, break"]
        );
        // u64 (string) -> u32 (number) is not a narrowing of the JSON form.
        let new_event = abi_with(
            json!({}),
            json!({ "NotFound": { "key": "string" }, "Gone": {} }),
            json!({ "Set": { "key": "string", "at": "u32" } }),
            json!({ "fields": {}, "version": 1 }),
        );
        assert_eq!(
            breaking(&diff(&old, &new_event)),
            ["event Set: field `at` changed: u64 -> u32"]
        );
    }

    #[test]
    fn state_without_a_version_bump_is_an_error() {
        let state = |fields: Value, version: u64| {
            abi_with(
                json!({}),
                json!({}),
                json!({}),
                json!({ "fields": fields, "version": version }),
            )
        };
        let old = state(json!({ "items": "table", "count": "counter" }), 3);

        let r = diff(
            &old,
            &state(json!({ "items": "unorderedMap", "count": "counter" }), 3),
        );
        assert!(r
            .state_error
            .as_deref()
            .is_some_and(|e| e.contains("`items` changed from table to unorderedMap")
                && e.contains("version = 4")));

        let r = diff(&old, &state(json!({ "count": "counter" }), 4));
        assert_eq!(r.state_error, None);
        assert!(r.to_string().contains("state: version 3 -> 4"));

        let r = diff(
            &old,
            &state(
                json!({ "items": "table", "count": "counter", "more": "cell" }),
                3,
            ),
        );
        assert_eq!(r.state_error, None);
        assert!(!r.is_breaking());

        let r = diff(
            &old,
            &state(json!({ "items": "table", "count": "counter" }), 5),
        );
        assert_eq!(
            breaking(&r),
            ["state: version 3 -> 5: the node upgrades one state version at a time"]
        );
        let r = diff(
            &old,
            &state(json!({ "items": "table", "count": "counter" }), 2),
        );
        assert_eq!(
            breaking(&r),
            ["state: version 3 -> 2: the node refuses a downgrade"]
        );
    }

    #[test]
    fn world_patch_is_compatible_minor_breaks() {
        let mut a = abi(json!({}));
        let mut b = a.clone();
        b.world = Some("calimero:app@0.5.2".into());
        assert!(!diff(&a, &b).is_breaking());
        a.world = Some("calimero:app@0.6.0".into());
        assert!(diff(&b, &a).is_breaking());
    }
}
