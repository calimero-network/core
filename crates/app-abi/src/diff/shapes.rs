//! The `types` section: each app type the old ABI's callers exchange, compared by name and
//! then by structure.
//!
//! A type's direction follows from where the old ABI uses it, through every type its shape
//! names: a parameter of a method called from outside is **sent** (the app reads it, so the new
//! shape must accept every old value), a result, error or event is **read** (clients decode it,
//! so the new shape must stay within the old one), and a type used both ways must keep both.
//!
//! - **Fields**: a sent record may gain a field callers may leave out (`option<..>` or
//!   `optional`), not a required one, and may not drop one; a read record may gain any field
//!   (every generated client ignores keys it does not know: TypeScript and Python read plain
//!   objects, Swift's `Decodable` skips them and the Kotlin client's `Json` has
//!   `ignoreUnknownKeys = true`), and may drop only one that was already `optional`.
//! - **Variants**: a sent enum may gain a variant, not lose one; a read enum may lose a variant,
//!   not gain one, since a client that decodes it exhaustively (a Swift or Kotlin enum, a
//!   `switch` over a TypeScript union) has no case for it.
//! - **Retyped** fields, payloads and items follow [`widens`]: wider when sent, narrower when
//!   read. A changed shape kind, tagging style, tuple length or generic parameter count breaks.
//! - **Known and unknown**: a type gaining a shape is compatible (clients typed it as any
//!   JSON); losing one breaks, since regenerated clients lose its type and the diff can no
//!   longer check it.
//!
//! Each type is compared once, by its entry name, so a recursive type (`Tree` holding
//! `list<Tree>`) or a generic one (`Page<T>`, reached as `Page<Item>`) is walked once per
//! direction: a generic type's own fields are compared with its parameters as they are named.

use std::collections::BTreeMap;

use super::{called_from_outside, widens, Diff, Severity};
use crate::abi::{AppAbi, EnumRepr, Field, Shape, TypeDef, Variant, VariantData};
use crate::ty::Type;

/// How the old ABI's callers exchange a type.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Use {
    /// Callers send it: a method parameter, or inside one.
    sent: bool,
    /// Callers read it: a result, error or event, or inside one.
    read: bool,
}

const SENT: Use = Use {
    sent: true,
    read: false,
};
const READ: Use = Use {
    sent: false,
    read: true,
};

impl Use {
    const fn covers(self, other: Self) -> bool {
        (self.sent || !other.sent) && (self.read || !other.read)
    }

    const fn with(self, other: Self) -> Self {
        Self {
            sent: self.sent || other.sent,
            read: self.read || other.read,
        }
    }
}

/// Each app type the old ABI's callers send or read, by entry name, through the shapes either
/// ABI gives it.
fn uses(old: &AppAbi, new: &AppAbi) -> BTreeMap<String, Use> {
    let mut work: Vec<(Type, Use)> = Vec::new();
    for m in old.methods.values().filter(|m| called_from_outside(m)) {
        work.extend(m.params.iter().map(|(_, t)| (t.clone(), SENT)));
        work.push((m.returns.clone(), READ));
    }
    for fields in old.errors.values().chain(old.events.values()) {
        work.extend(fields.values().map(|t| (t.clone(), READ)));
    }
    let mut out: BTreeMap<String, Use> = BTreeMap::new();
    while let Some((ty, u)) = work.pop() {
        let mut named = Vec::new();
        nested_named(&ty, &mut named);
        for t in named {
            let Some((name, args)) = t.named_parts() else {
                continue;
            };
            work.extend(args.into_iter().map(|a| (a, u)));
            let seen = out.entry(name.clone()).or_default();
            if seen.covers(u) {
                continue;
            }
            *seen = seen.with(u);
            for def in [old.types.get(&name), new.types.get(&name)]
                .into_iter()
                .flatten()
            {
                // The parameters are the arguments' business, pushed above.
                let blank = vec![Type::Unit; def.params.len()];
                work.extend(
                    shape_types(&def.shape)
                        .into_iter()
                        .map(|t| (t.substitute(&def.params, &blank), u)),
                );
            }
        }
    }
    out
}

/// Every [`Type::Named`] inside `ty`, itself included.
fn nested_named<'t>(ty: &'t Type, out: &mut Vec<&'t Type>) {
    match ty {
        Type::Named(_) => out.push(ty),
        Type::Option(t) | Type::List(t) | Type::Set(t) | Type::Array(t, _) => {
            nested_named(t, out);
        }
        Type::Map(k, v) => {
            nested_named(k, out);
            nested_named(v, out);
        }
        Type::Tuple(ts) => ts.iter().for_each(|t| nested_named(t, out)),
        _ => {}
    }
}

/// The types a shape names directly.
fn shape_types(shape: &Shape) -> Vec<&Type> {
    fn fields(fs: &[Field]) -> Vec<&Type> {
        fs.iter().map(|f| &f.ty).collect()
    }
    match shape {
        Shape::Record(fs) => fields(fs),
        Shape::Newtype(t) => vec![t],
        Shape::Tuple(ts) => ts.iter().collect(),
        Shape::Unit => Vec::new(),
        Shape::Enum { variants, .. } => variants
            .iter()
            .flat_map(|v| match &v.data {
                VariantData::Unit => Vec::new(),
                VariantData::Newtype(t) => vec![t],
                VariantData::Tuple(ts) => ts.iter().collect(),
                VariantData::Record(fs) => fields(fs),
            })
            .collect(),
    }
}

fn shape_kind(shape: &Shape) -> &'static str {
    match shape {
        Shape::Record(_) => "record",
        Shape::Newtype(_) => "newtype",
        Shape::Tuple(_) => "tuple",
        Shape::Unit => "unit",
        Shape::Enum { .. } => "enum",
    }
}

fn data_kind(data: &VariantData) -> &'static str {
    match data {
        VariantData::Unit => "unit",
        VariantData::Newtype(_) => "newtype",
        VariantData::Tuple(_) => "tuple",
        VariantData::Record(_) => "record",
    }
}

fn repr_text(repr: &EnumRepr) -> String {
    match repr {
        EnumRepr::External => "external".to_owned(),
        EnumRepr::Internal { tag } => format!("internal (tag `{tag}`)"),
        EnumRepr::Adjacent { tag, content } => {
            format!("adjacent (tag `{tag}`, content `{content}`)")
        }
        EnumRepr::Untagged => "untagged".to_owned(),
    }
}

fn params_text(params: &[String]) -> String {
    if params.is_empty() {
        "none".to_owned()
    } else {
        format!("<{}>", params.join(", "))
    }
}

/// Whether a caller may leave the field out of what it sends: serde reads a missing `Option`
/// as `None`, and an `optional` field is one the app's own type writes without, so reads back
/// without.
const fn may_omit(f: &Field) -> bool {
    f.optional || f.ty.is_optional()
}

impl Diff {
    /// Changes inside the shapes of the types the old ABI's callers exchange.
    pub(super) fn types(&mut self, old: &AppAbi, new: &AppAbi) {
        for (name, u) in uses(old, new) {
            let mut c = Compare {
                d: self,
                subject: format!("type {name}"),
                u,
            };
            match (old.types.get(&name), new.types.get(&name)) {
                (None, None) => {}
                (None, Some(_)) => c.push(
                    Severity::Compatible,
                    "shape now described (clients typed it as any JSON)",
                ),
                (Some(_), None) => c.push(
                    Severity::Breaking,
                    "shape no longer described: clients generated from the new ABI lose its type, and its changes go unchecked",
                ),
                (Some(o), Some(n)) => c.def(o, n),
            }
        }
    }
}

/// One type's comparison.
struct Compare<'d> {
    d: &'d mut Diff,
    subject: String,
    u: Use,
}

impl Compare<'_> {
    fn push(&mut self, severity: Severity, message: impl Into<String>) {
        self.d.push(severity, self.subject.clone(), message);
    }

    /// A change that holds for senders when `send_ok` and for readers when `read_ok`; the
    /// reason names the side it breaks.
    fn change(&mut self, what: String, send_ok: bool, read_ok: bool, why: (&str, &str)) {
        let (severity, message) = if self.u.sent && !send_ok {
            (Severity::Breaking, format!("{what}: {}", why.0))
        } else if self.u.read && !read_ok {
            (Severity::Breaking, format!("{what}: {}", why.1))
        } else {
            (Severity::Compatible, what)
        };
        self.push(severity, message);
    }

    fn def(&mut self, o: &TypeDef, n: &TypeDef) {
        if o.params.len() != n.params.len() {
            self.push(
                Severity::Breaking,
                format!(
                    "generic parameters {} -> {}",
                    params_text(&o.params),
                    params_text(&n.params)
                ),
            );
            return;
        }
        let renamed = o.params != n.params;
        if renamed {
            self.push(
                Severity::Compatible,
                format!(
                    "generic parameters renamed {} -> {}",
                    params_text(&o.params),
                    params_text(&n.params)
                ),
            );
        }
        let old_names: Vec<Type> = o.params.iter().cloned().map(Type::Named).collect();
        let n_shape = if renamed {
            n.instantiate(&old_names)
        } else {
            n.shape.clone()
        };
        self.shape(&o.shape, &n_shape);
    }

    fn shape(&mut self, o: &Shape, n: &Shape) {
        match (o, n) {
            (Shape::Record(of), Shape::Record(nf)) => self.fields("", of, nf),
            (Shape::Newtype(ot), Shape::Newtype(nt)) => self.ty("inner type", ot, nt),
            (Shape::Tuple(ot), Shape::Tuple(nt)) => self.items("", ot, nt),
            (Shape::Unit, Shape::Unit) => {}
            (
                Shape::Enum {
                    repr: orepr,
                    variants: ov,
                },
                Shape::Enum {
                    repr: nrepr,
                    variants: nv,
                },
            ) => {
                if orepr != nrepr {
                    self.push(
                        Severity::Breaking,
                        format!(
                            "tagging {} -> {}: the JSON of every variant changes",
                            repr_text(orepr),
                            repr_text(nrepr)
                        ),
                    );
                }
                self.variants(nrepr, ov, nv);
            }
            _ => self.push(
                Severity::Breaking,
                format!("shape {} -> {}", shape_kind(o), shape_kind(n)),
            ),
        }
    }

    /// `what` changed from `o` to `n`: senders need it to widen, readers to narrow.
    fn ty(&mut self, what: &str, o: &Type, n: &Type) {
        if o == n {
            return;
        }
        let (wider, narrower) = (widens(o, n), widens(n, o));
        let how = match (wider, narrower) {
            (true, false) => "widened",
            (false, true) => "narrowed",
            _ => "changed",
        };
        self.change(
            format!("{what} {how}: {o} -> {n}"),
            wider,
            narrower,
            (
                "values callers send in the old type are refused",
                "clients get values outside the old type",
            ),
        );
    }

    fn items(&mut self, at: &str, o: &[Type], n: &[Type]) {
        if o.len() != n.len() {
            self.push(
                Severity::Breaking,
                format!("{at}{} items -> {}", o.len(), n.len()),
            );
            return;
        }
        for (i, (ot, nt)) in o.iter().zip(n).enumerate() {
            self.ty(&format!("{at}item {i}"), ot, nt);
        }
    }

    fn fields(&mut self, at: &str, o: &[Field], n: &[Field]) {
        let old: BTreeMap<&str, &Field> = o.iter().map(|f| (f.name.as_str(), f)).collect();
        let new: BTreeMap<&str, &Field> = n.iter().map(|f| (f.name.as_str(), f)).collect();
        for (name, of) in &old {
            let Some(nf) = new.get(name) else {
                self.change(
                    format!("{at}field `{name}` removed"),
                    false,
                    of.optional,
                    (
                        "callers that send it are no longer heard",
                        "clients that read it break",
                    ),
                );
                continue;
            };
            if of.optional != nf.optional {
                let what = if nf.optional {
                    format!("{at}field `{name}` may now be left out")
                } else {
                    format!("{at}field `{name}` is now always written")
                };
                self.change(
                    what,
                    !may_omit(of) || may_omit(nf),
                    of.optional || !nf.optional,
                    (
                        "callers that leave it out fail",
                        "clients that expect it break",
                    ),
                );
            }
            self.ty(&format!("{at}field `{name}`"), &of.ty, &nf.ty);
        }
        for (name, nf) in &new {
            if old.contains_key(name) {
                continue;
            }
            let required = if may_omit(nf) { "optional" } else { "required" };
            self.change(
                format!("{at}{required} field `{name}` added ({})", nf.ty),
                may_omit(nf),
                true,
                ("callers that leave it out fail", ""),
            );
        }
        let order = |fs: &[Field]| fs.iter().map(|f| f.name.clone()).collect::<Vec<_>>();
        if old.keys().eq(new.keys()) && order(o) != order(n) {
            self.push(
                Severity::Compatible,
                format!("{at}fields reordered (JSON names them)"),
            );
        }
    }

    fn variants(&mut self, repr: &EnumRepr, o: &[Variant], n: &[Variant]) {
        let old: BTreeMap<&str, &Variant> = o.iter().map(|v| (v.name.as_str(), v)).collect();
        let new: BTreeMap<&str, &Variant> = n.iter().map(|v| (v.name.as_str(), v)).collect();
        for (name, ov) in &old {
            let Some(nv) = new.get(name) else {
                self.change(
                    format!("variant `{name}` removed"),
                    false,
                    true,
                    ("callers that send it are refused", ""),
                );
                continue;
            };
            let at = format!("variant `{name}` ");
            match (&ov.data, &nv.data) {
                (VariantData::Unit, VariantData::Unit) => {}
                (VariantData::Newtype(ot), VariantData::Newtype(nt)) => {
                    self.ty(&format!("{at}payload"), ot, nt);
                }
                (VariantData::Tuple(ot), VariantData::Tuple(nt)) => self.items(&at, ot, nt),
                (VariantData::Record(of), VariantData::Record(nf)) => self.fields(&at, of, nf),
                (od, nd) => self.push(
                    Severity::Breaking,
                    format!("{at}payload {} -> {}", data_kind(od), data_kind(nd)),
                ),
            }
        }
        for name in new.keys().filter(|k| !old.contains_key(*k)) {
            self.change(
                format!("variant `{name}` added"),
                true,
                false,
                (
                    "",
                    "a client that decodes the enum exhaustively (Swift, Kotlin, a TypeScript switch) has no case for it",
                ),
            );
        }
        let order = |vs: &[Variant]| vs.iter().map(|v| v.name.clone()).collect::<Vec<_>>();
        if old.keys().eq(new.keys()) && order(o) != order(n) {
            if *repr == EnumRepr::Untagged {
                self.change(
                    "variants reordered".to_owned(),
                    false,
                    true,
                    (
                        "serde tries untagged variants in order, so a value callers send may read as another variant",
                        "",
                    ),
                );
            } else {
                self.push(Severity::Compatible, "variants reordered");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use crate::diff::{diff, Report, Severity};
    use crate::AppAbi;

    /// An ABI whose method `send` takes `In`, view `read` returns `Out`, `both` takes and
    /// returns `Both`, and `internal` (called by no one outside) takes `Hidden`.
    fn abi(types: Value) -> AppAbi {
        AppAbi::from_json(&json!({
            "abiVersion": 1,
            "methods": {
                "send": { "kind": "call", "params": [["v", "In"]] },
                "read": { "kind": "view", "params": [], "returns": "option<list<Out>>" },
                "both": { "kind": "call", "params": [["v", "Both"]], "returns": "Both" },
                "internal": { "kind": "call", "params": [["v", "Hidden"]], "allowAction": true },
            },
            "events": { "Seen": { "what": "Ev" } },
            "errors": { "Bad": { "why": "BTreeMap<String,Er>" } },
            "types": types,
        }))
        .unwrap()
    }

    fn lines(r: &Report, severity: Severity) -> Vec<String> {
        let mut out: Vec<String> = r
            .changes
            .iter()
            .filter(|c| c.severity == severity)
            .map(|c| format!("{}: {}", c.subject, c.message))
            .collect();
        out.sort();
        out
    }

    fn record(fields: Value) -> Value {
        json!({ "kind": "record", "fields": fields })
    }

    fn enumeration(variants: Value) -> Value {
        json!({ "kind": "enum", "variants": variants })
    }

    /// The same change applied to the sent, read and two-way records.
    fn each_way(old: &Value, new: &Value) -> Report {
        let all =
            |r: &Value| json!({ "In": r, "Out": r, "Both": r, "Ev": r, "Er": r, "Hidden": r });
        diff(&abi(all(old)), &abi(all(new)))
    }

    #[test]
    fn an_unchanged_shape_has_no_changes() {
        let t = record(json!([["a", "u32"]]));
        let r = each_way(&t, &t);
        assert!(r.changes.is_empty(), "{r}");
    }

    #[test]
    fn a_field_added() {
        let old = record(json!([["a", "u32"]]));
        let r = each_way(&old, &record(json!([["a", "u32"], ["b", "string"]])));
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: required field `b` added (string): callers that leave it out fail",
                "type In: required field `b` added (string): callers that leave it out fail",
            ]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            [
                "type Er: required field `b` added (string)",
                "type Ev: required field `b` added (string)",
                "type Out: required field `b` added (string)",
            ]
        );
        // `option<..>` or `optional`: senders may leave it out.
        for added in [
            json!(["b", "option<string>"]),
            json!(["b", "string", { "optional": true }]),
        ] {
            let r = each_way(&old, &record(json!([["a", "u32"], added])));
            assert!(!r.is_breaking(), "{r}");
            assert_eq!(r.changes.len(), 5, "{r}");
        }
    }

    #[test]
    fn a_field_removed() {
        let r = each_way(
            &record(json!([["a", "u32"], ["b", "string"]])),
            &record(json!([["a", "u32"]])),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: field `b` removed: callers that send it are no longer heard",
                "type Er: field `b` removed: clients that read it break",
                "type Ev: field `b` removed: clients that read it break",
                "type In: field `b` removed: callers that send it are no longer heard",
                "type Out: field `b` removed: clients that read it break",
            ]
        );
        // Readers already handled a missing `optional` field.
        let r = each_way(
            &record(json!([["a", "u32"], ["b", "string", { "optional": true }]])),
            &record(json!([["a", "u32"]])),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: field `b` removed: callers that send it are no longer heard",
                "type In: field `b` removed: callers that send it are no longer heard",
            ]
        );
    }

    #[test]
    fn a_field_retyped_widens_when_sent_and_narrows_when_read() {
        let r = each_way(
            &record(json!([["a", "u16"]])),
            &record(json!([["a", "u32"]])),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: field `a` widened: u16 -> u32: clients get values outside the old type",
                "type Er: field `a` widened: u16 -> u32: clients get values outside the old type",
                "type Ev: field `a` widened: u16 -> u32: clients get values outside the old type",
                "type Out: field `a` widened: u16 -> u32: clients get values outside the old type",
            ]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["type In: field `a` widened: u16 -> u32"]
        );
        let r = each_way(
            &record(json!([["a", "option<string>"]])),
            &record(json!([["a", "string"]])),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: field `a` narrowed: option<string> -> string: values callers send in the old type are refused",
                "type In: field `a` narrowed: option<string> -> string: values callers send in the old type are refused",
            ]
        );
        // A number becoming a decimal string holds neither way.
        let r = each_way(
            &record(json!([["a", "u32"]])),
            &record(json!([["a", "u64"]])),
        );
        assert_eq!(lines(&r, Severity::Breaking).len(), 5, "{r}");
        assert!(r
            .to_string()
            .contains("type In: field `a` changed: u32 -> u64"));
    }

    #[test]
    fn a_field_optional_or_required() {
        let required = record(json!([["a", "string"]]));
        let optional = record(json!([["a", "string", { "optional": true }]]));
        let r = each_way(&required, &optional);
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: field `a` may now be left out: clients that expect it break",
                "type Er: field `a` may now be left out: clients that expect it break",
                "type Ev: field `a` may now be left out: clients that expect it break",
                "type Out: field `a` may now be left out: clients that expect it break",
            ]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["type In: field `a` may now be left out"]
        );
        let r = each_way(&optional, &required);
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: field `a` is now always written: callers that leave it out fail",
                "type In: field `a` is now always written: callers that leave it out fail",
            ]
        );
        // An `option<..>` field stays one a sender may leave out.
        let r = each_way(
            &record(json!([["a", "option<string>", { "optional": true }]])),
            &record(json!([["a", "option<string>"]])),
        );
        assert!(!r.is_breaking(), "{r}");
    }

    #[test]
    fn fields_reordered_are_compatible() {
        let r = each_way(
            &record(json!([["a", "u32"], ["b", "u32"]])),
            &record(json!([["b", "u32"], ["a", "u32"]])),
        );
        assert!(!r.is_breaking(), "{r}");
        assert!(r
            .to_string()
            .contains("type In: fields reordered (JSON names them)"));
    }

    #[test]
    fn a_variant_added_or_removed() {
        let two = enumeration(json!([{ "name": "A" }, { "name": "B" }]));
        let three = enumeration(json!([{ "name": "A" }, { "name": "B" }, { "name": "C" }]));
        let r = each_way(&two, &three);
        let exhaustive = "a client that decodes the enum exhaustively (Swift, Kotlin, a TypeScript switch) has no case for it";
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["Both", "Er", "Ev", "Out"]
                .map(|t| format!("type {t}: variant `C` added: {exhaustive}"))
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["type In: variant `C` added"]
        );
        let r = each_way(&three, &two);
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type Both: variant `C` removed: callers that send it are refused",
                "type In: variant `C` removed: callers that send it are refused",
            ]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["Er", "Ev", "Out"].map(|t| format!("type {t}: variant `C` removed"))
        );
    }

    #[test]
    fn variant_payloads() {
        let old = enumeration(json!([
            { "name": "N", "type": "u16" },
            { "name": "T", "items": ["u32", "string"] },
            { "name": "R", "fields": [["at", "u32"]] },
            { "name": "K" },
        ]));
        let new = enumeration(json!([
            { "name": "N", "type": "u32" },
            { "name": "T", "items": ["u32", "string", "bool"] },
            { "name": "R", "fields": [["at", "u32"], ["by", "string"]] },
            { "name": "K", "type": "string" },
        ]));
        let types = |t: &Value| json!({ "In": t, "Out": t });
        let r = diff(&abi(types(&old)), &abi(types(&new)));
        assert_eq!(
            lines(&r, Severity::Breaking),
            [
                "type In: variant `K` payload unit -> newtype",
                "type In: variant `R` required field `by` added (string): callers that leave it out fail",
                "type In: variant `T` 2 items -> 3",
                "type Out: variant `K` payload unit -> newtype",
                "type Out: variant `N` payload widened: u16 -> u32: clients get values outside the old type",
                "type Out: variant `T` 2 items -> 3",
            ]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            [
                "type In: variant `N` payload widened: u16 -> u32",
                "type Out: variant `R` required field `by` added (string)",
            ]
        );
    }

    #[test]
    fn a_tagging_style_change_breaks() {
        let variants = json!([{ "name": "A" }, { "name": "B", "fields": [["x", "u32"]] }]);
        let styles = [
            json!({ "kind": "enum", "variants": variants }),
            json!({ "kind": "enum", "tag": "type", "variants": variants }),
            json!({ "kind": "enum", "tag": "t", "content": "c", "variants": variants }),
            json!({ "kind": "enum", "untagged": true, "variants": variants }),
        ];
        for (i, o) in styles.iter().enumerate() {
            for (j, n) in styles.iter().enumerate() {
                let r = diff(&abi(json!({ "Out": o })), &abi(json!({ "Out": n })));
                assert_eq!(r.is_breaking(), i != j, "{i} -> {j}: {r}");
            }
        }
        let r = diff(
            &abi(json!({ "In": styles[0] })),
            &abi(json!({ "In": styles[2] })),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["type In: tagging external -> adjacent (tag `t`, content `c`): the JSON of every variant changes"]
        );
    }

    #[test]
    fn untagged_variants_reordered_break_senders() {
        let ab = json!({ "kind": "enum", "untagged": true, "variants": [{ "name": "A", "type": "u32" }, { "name": "B", "type": "string" }] });
        let ba = json!({ "kind": "enum", "untagged": true, "variants": [{ "name": "B", "type": "string" }, { "name": "A", "type": "u32" }] });
        let r = diff(
            &abi(json!({ "In": ab, "Out": ab })),
            &abi(json!({ "In": ba, "Out": ba })),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["type In: variants reordered: serde tries untagged variants in order, so a value callers send may read as another variant"]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["type Out: variants reordered"]
        );
        let tagged = |v: &Value| json!({ "kind": "enum", "variants": v["variants"] });
        let r = diff(
            &abi(json!({ "In": tagged(&ab) })),
            &abi(json!({ "In": tagged(&ba) })),
        );
        assert!(!r.is_breaking(), "{r}");
    }

    #[test]
    fn a_shape_kind_change_breaks() {
        let r = each_way(
            &json!({ "kind": "newtype", "type": "string" }),
            &record(json!([["value", "string"]])),
        );
        assert_eq!(lines(&r, Severity::Breaking).len(), 5, "{r}");
        assert!(r.to_string().contains("type In: shape newtype -> record"));
        let r = each_way(
            &json!({ "kind": "newtype", "type": "u16" }),
            &json!({ "kind": "newtype", "type": "u32" }),
        );
        assert!(r
            .to_string()
            .contains("compatible type In: inner type widened: u16 -> u32"));
        let r = each_way(
            &json!({ "kind": "tuple", "items": ["f64", "f64"] }),
            &json!({ "kind": "tuple", "items": ["f64", "f64", "f64"] }),
        );
        assert!(r.to_string().contains("type Out: 2 items -> 3"));
    }

    #[test]
    fn a_shape_gained_is_compatible_and_one_lost_breaks() {
        let t = record(json!([["a", "u32"]]));
        let r = diff(&abi(json!({})), &abi(json!({ "In": t, "Out": t })));
        assert!(!r.is_breaking(), "{r}");
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["In", "Out"]
                .map(|n| format!("type {n}: shape now described (clients typed it as any JSON)"))
        );
        let r = diff(&abi(json!({ "In": t, "Out": t })), &abi(json!({})));
        assert_eq!(lines(&r, Severity::Breaking).len(), 2, "{r}");
        assert!(r
            .to_string()
            .contains("type Out: shape no longer described"));
    }

    #[test]
    fn types_no_caller_exchanges_are_not_compared() {
        let r = diff(
            &abi(json!({ "Hidden": record(json!([["a", "u32"]])), "Spare": record(json!([])) })),
            &abi(json!({ "Hidden": record(json!([["b", "u32"]])) })),
        );
        assert!(r.changes.is_empty(), "{r}");
    }

    #[test]
    fn nested_types_take_the_direction_they_travel() {
        // `Out` holds `Inner`, which is read; `In` holds `Page<Arg>`, so `Page` and `Arg` are
        // sent.
        let types = |inner: Value, arg: Value| {
            json!({
                "Out": record(json!([["inner", "option<Inner>"]])),
                "Inner": inner,
                "In": record(json!([["page", "Page<Arg>"]])),
                "Page": { "kind": "record", "params": ["T"], "fields": [["items", "list<T>"]] },
                "Arg": arg,
            })
        };
        let old = types(record(json!([["a", "u32"]])), record(json!([["a", "u32"]])));
        let new = types(
            record(json!([["a", "u32"], ["b", "u32"]])),
            record(json!([["a", "u32"], ["b", "u32"]])),
        );
        let r = diff(&abi(old), &abi(new));
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["type Arg: required field `b` added (u32): callers that leave it out fail"]
        );
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["type Inner: required field `b` added (u32)"]
        );
    }

    #[test]
    fn generic_parameters() {
        let page = |params: Value, fields: Value| json!({ "Out": record(json!([["p", "Page<u32>"]])), "Page": { "kind": "record", "params": params, "fields": fields } });
        let old = page(json!(["T"]), json!([["items", "list<T>"]]));
        let r = diff(
            &abi(old.clone()),
            &abi(page(
                json!(["T", "U"]),
                json!([["items", "list<T>"], ["more", "U"]]),
            )),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["type Page: generic parameters <T> -> <T, U>"]
        );
        let r = diff(
            &abi(old.clone()),
            &abi(page(json!(["Item"]), json!([["items", "list<Item>"]]))),
        );
        assert!(!r.is_breaking(), "{r}");
        assert_eq!(
            lines(&r, Severity::Compatible),
            ["type Page: generic parameters renamed <T> -> <Item>"]
        );
        let r = diff(
            &abi(old),
            &abi(page(json!(["T"]), json!([["items", "option<T>"]]))),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["type Page: field `items` changed: list<T> -> option<T>: clients get values outside the old type"]
        );
    }

    #[test]
    fn recursive_types_end() {
        let tree = |extra: Value| {
            json!({
                "Out": { "kind": "record", "fields": [["value", "u32"], ["children", "list<Out>"], ["other", "option<Node>"]] },
                "Node": { "kind": "enum", "variants": [{ "name": "Leaf" }, { "name": "Branch", "type": "Out" }, extra] },
            })
        };
        let r = diff(
            &abi(tree(json!({ "name": "Self", "type": "Node" }))),
            &abi(tree(json!({ "name": "Self", "type": "list<Node>" }))),
        );
        assert_eq!(
            lines(&r, Severity::Breaking),
            ["type Node: variant `Self` payload changed: Node -> list<Node>: clients get values outside the old type"]
        );
    }
}
