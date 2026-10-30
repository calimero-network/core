//! Support for the code the `app` macros generate. Not a public API: names here change
//! with the macros.

use core::fmt::Display;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{json, Value};

use crate::env::{self, Caller, TimerInfo};
use crate::error::{AppError, Result};

pub use serde;
pub use serde_json;

#[cfg(target_arch = "wasm32")]
pub use crate::bindings;

/// What a method is, as `#[app::logic]` declared it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodKind {
    /// `#[app::init]`.
    Init,
    /// A `&mut self` or plain `&self` method without `view`.
    Call,
    /// `#[app::view]`.
    View,
    /// `#[app::every(..)]`.
    Timer,
    /// `#[app::compute(..)]`: runs on a job node as an action.
    Compute,
}

/// One exposed method.
#[derive(Clone, Copy, Debug)]
pub struct Method {
    /// The ABI (camelCase) name.
    pub name: &'static str,
    /// What it is.
    pub kind: MethodKind,
    /// `#[app::public]`: callable without an account.
    pub public: bool,
    /// `#[app::allow(system)]`.
    pub allow_system: bool,
    /// `#[app::allow(app = "..")]`: apps whose delivered xcalls it accepts.
    pub allow_apps: &'static [&'static str],
}

/// The invocation kind of `handle`, mirroring the WIT `invocation-kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A logged call.
    Call,
    /// A read on any node.
    View,
    /// A compute run.
    Action,
    /// A timer tick; `args` carry the tick.
    Timer,
    /// A delivered cross-app call.
    XcallDeliver,
    /// The one-time initializer.
    Init,
    /// A migration batch (not in this SDK version).
    Migrate,
}

/// Implemented by `#[app::logic]` for the state type.
pub trait Dispatch {
    /// The exposed methods, in declaration order.
    const METHODS: &'static [Method];
    /// Run method `index` of [`Dispatch::METHODS`] with its JSON arguments.
    fn run(index: usize, args: &[u8]) -> Result<Vec<u8>>;
}

/// The timer tick the host passes as `args` for `kind = timer`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Tick {
    #[serde(with = "crate::json::u64_str")]
    scheduled: u64,
    #[serde(default, with = "crate::json::opt_u64_str")]
    previous: Option<u64>,
}

fn bad_args(message: impl Display) -> AppError {
    AppError::with_fields("BadArgs", json!({ "message": message.to_string() }))
}

/// The generated `handle`: find the method, check the invocation kind and the caller (D13),
/// then run it.
///
/// A view may be invoked as a call; a call, timer or init method invoked as a view is
/// `NotAView`; any other kind mismatch is `WrongKind`. A `System` caller needs
/// `allow(system)`, a `Public` one `public`, an `App` one `allow(app = ..)`; `init` runs as
/// the owner and is not checked.
pub fn handle<D: Dispatch>(kind: Kind, method: &str, args: &[u8]) -> Result<Vec<u8>> {
    env::set_timer(None);
    let Some(index) = D::METHODS.iter().position(|m| m.name == method) else {
        return Err(AppError::with_fields(
            "UnknownMethod",
            json!({ "method": method }),
        ));
    };
    let m = &D::METHODS[index];
    match (kind, m.kind) {
        (Kind::View, MethodKind::View)
        | (Kind::Call | Kind::XcallDeliver, MethodKind::Call | MethodKind::View)
        | (Kind::Timer, MethodKind::Timer)
        | (Kind::Init, MethodKind::Init)
        | (Kind::Action, MethodKind::Compute) => {}
        (Kind::View, _) => return Err(AppError::new("NotAView")),
        _ => return Err(AppError::new("WrongKind")),
    }
    let args = if kind == Kind::Timer {
        let tick: Tick = decode_args(args)?;
        env::set_timer(Some(TimerInfo {
            scheduled: tick.scheduled,
            previous: tick.previous,
        }));
        b"{}".as_slice()
    } else {
        args
    };
    // The node vets the caller when it starts a compute run, so the run itself is not checked.
    if kind != Kind::Init && m.kind != MethodKind::Compute {
        match env::caller() {
            Caller::Account(_) | Caller::ApiKey { .. } => {}
            Caller::System if m.allow_system => {}
            Caller::Public if m.public => {}
            Caller::App(app) if m.allow_apps.contains(&app.as_str()) => {}
            Caller::Public => return Err(AppError::login_required()),
            Caller::System | Caller::App(_) => return Err(AppError::forbidden()),
        }
    }
    D::run(index, args)
}

/// Decode a JSON object of arguments; empty `args` is `{}`. Anything else is `BadArgs`.
pub fn decode_args<T: DeserializeOwned>(args: &[u8]) -> Result<T> {
    let value: Value = if args.is_empty() {
        Value::Object(serde_json::Map::new())
    } else {
        serde_json::from_slice(args).map_err(bad_args)?
    };
    if !value.is_object() {
        return Err(bad_args("arguments are a JSON object"));
    }
    serde_json::from_value(value).map_err(bad_args)
}

/// Encode a result as JSON.
pub fn encode<T: Serialize + ?Sized>(value: &T) -> Result<Vec<u8>> {
    serde_json::to_vec(value)
        .map_err(|e| AppError::with_fields("Encode", json!({ "message": e.to_string() })))
}

/// A JSON value of an error or event field.
pub fn to_json_value<T: Serialize + ?Sized>(value: &T) -> Result<Value> {
    serde_json::to_value(value)
        .map_err(|e| AppError::with_fields("Encode", json!({ "message": e.to_string() })))
}

/// The code and JSON fields of an error or event value.
pub fn payload<T>(
    value: &T,
    f: impl FnOnce(&T) -> Result<(&'static str, serde_json::Map<String, Value>)>,
) -> Result<(&'static str, Value)> {
    let (name, fields) = f(value)?;
    Ok((name, Value::Object(fields)))
}

/// A 64-bit integer written as a decimal string (D20).
pub struct Decimal<T>(pub T);

impl<T: Display> Serialize for Decimal<T> {
    fn serialize<S: Serializer>(&self, s: S) -> core::result::Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

/// Bytes written as base64url without padding (D20).
pub struct Base64Url<T>(pub T);

impl<T: AsRef<[u8]>> Serialize for Base64Url<T> {
    fn serialize<S: Serializer>(&self, s: S) -> core::result::Result<S::Ok, S::Error> {
        s.serialize_str(&base64url(&self.0))
    }
}

/// `bytes` as base64url without padding.
pub fn base64url<T: AsRef<[u8]> + ?Sized>(bytes: &T) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The `message` of a wire `app-error`: the error's fields as JSON, `{}` when it has none.
pub fn error_message(error: &AppError) -> String {
    if error.fields.is_null() {
        "{}".to_owned()
    } else {
        error.fields.to_string()
    }
}

/// The ABI parts used when a module declares no `#[app::error]` or `#[app::event]`; an
/// enum declared in the module shadows them (an explicit item beats a glob import).
pub mod abi_defaults {
    /// No declared errors.
    pub const __CALIMERO_ABI_ERRORS: &str = "{}";
    /// No declared events.
    pub const __CALIMERO_ABI_EVENTS: &str = "{}";
}

/// The total length of the ABI parts.
pub const fn abi_len(parts: &[&str]) -> usize {
    let mut len = 0;
    let mut i = 0;
    while i < parts.len() {
        len += parts[i].len();
        i += 1;
    }
    len
}

/// The ABI parts concatenated; `N` is [`abi_len`] of them.
pub const fn abi_concat<const N: usize>(parts: &[&str]) -> [u8; N] {
    let mut out = [0u8; N];
    let mut at = 0;
    let mut i = 0;
    while i < parts.len() {
        let bytes = parts[i].as_bytes();
        let mut j = 0;
        while j < bytes.len() {
            out[at] = bytes[j];
            at += 1;
            j += 1;
        }
        i += 1;
    }
    out
}

/// The `handle` export of a state type `D`, for `__calimero_export_handle!`.
#[cfg(target_arch = "wasm32")]
pub struct Export<D>(core::marker::PhantomData<D>);

#[cfg(target_arch = "wasm32")]
impl<D: Dispatch> bindings::Guest for Export<D> {
    fn handle(
        kind: bindings::InvocationKind,
        method: String,
        args: Vec<u8>,
    ) -> core::result::Result<Vec<u8>, bindings::AppError> {
        let kind = match kind {
            bindings::InvocationKind::Call => Kind::Call,
            bindings::InvocationKind::View => Kind::View,
            bindings::InvocationKind::Action => Kind::Action,
            bindings::InvocationKind::Timer => Kind::Timer,
            bindings::InvocationKind::XcallDeliver => Kind::XcallDeliver,
            bindings::InvocationKind::Init => Kind::Init,
            bindings::InvocationKind::Migrate => Kind::Migrate,
        };
        handle::<D>(kind, &method, &args).map_err(|e| bindings::AppError {
            message: error_message(&e),
            code: e.code,
        })
    }
}
