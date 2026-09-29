//! What a client key was minted *for*.
//!
//! A client key is issued by the node owner to one of their applications, and
//! the server treats it as the node owner (`AuthenticatedNodeOwner`). Its
//! permission list says *which routes* it may call, but for `/jsonrpc` that is
//! path-only: `context:execute` reaches every context on the node. A key minted
//! for one app therefore reached every other app's contexts too.
//!
//! Bindings narrow that. They are recorded in the key's permission list at mint
//! time (so they travel inside the token and cost no extra lookup):
//!
//! * `context[<context_id>,<identity>]` — the single-context binding
//!   `/admin/client-key` has always prepended when a context is chosen, and
//!   which nothing enforced until now;
//! * `application-binding[<application_id>]` — added when the mint request
//!   names the application the key is for.
//!
//! The permission validator ignores both for route authorization: the first
//! only ever narrowed, and the second does not parse as a `Permission` at all
//! (unparseable entries are skipped), so a binding can never *grant* anything.

/// Permission-list marker for the application a client key is bound to.
pub const APPLICATION_BINDING: &str = "application-binding";

/// The marker recorded for a key bound to `application_id`.
#[must_use]
pub fn application_binding(application_id: &str) -> String {
    format!("{APPLICATION_BINDING}[{application_id}]")
}

/// The contexts and application a client key may act on. Empty (`is_unbound`)
/// for keys minted without either — those keep today's behaviour.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientKeyBindings {
    /// Set by a single-context mint: the only context the key may act on.
    pub context_id: Option<String>,
    /// Set when the mint named its application: only that app's contexts.
    pub application_id: Option<String>,
}

impl ClientKeyBindings {
    /// Read the bindings out of a key's (or token's) permission list.
    #[must_use]
    pub fn from_permissions(permissions: &[String]) -> Self {
        let mut bindings = Self::default();

        for permission in permissions {
            if let Some(inner) = bracketed(permission, "context") {
                // `context[<ctx>,<identity>]`: the context is the first param.
                let ctx = inner.split(',').next().unwrap_or("").trim();
                if !ctx.is_empty() && bindings.context_id.is_none() {
                    bindings.context_id = Some(ctx.to_owned());
                }
            } else if let Some(inner) = bracketed(permission, APPLICATION_BINDING) {
                let app = inner.trim();
                if !app.is_empty() && bindings.application_id.is_none() {
                    bindings.application_id = Some(app.to_owned());
                }
            }
        }

        bindings
    }

    /// No binding at all: a key minted before bindings existed, or without
    /// naming a context or application.
    #[must_use]
    pub const fn is_unbound(&self) -> bool {
        self.context_id.is_none() && self.application_id.is_none()
    }

    /// May this key act on `context_id`, which runs `application_id`?
    #[must_use]
    pub fn permits(&self, context_id: &str, application_id: &str) -> bool {
        self.context_id.as_deref().is_none_or(|c| c == context_id)
            && self
                .application_id
                .as_deref()
                .is_none_or(|a| a == application_id)
    }
}

/// `name[inner]` → `inner`, for exactly that name (so `context:execute[x]` is
/// not mistaken for the `context[x,y]` binding).
fn bracketed<'a>(permission: &'a str, name: &str) -> Option<&'a str> {
    permission
        .strip_prefix(name)?
        .strip_prefix('[')?
        .strip_suffix(']')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn perms(list: &[&str]) -> Vec<String> {
        list.iter().map(|p| (*p).to_owned()).collect()
    }

    #[test]
    fn reads_the_single_context_binding() {
        let b = ClientKeyBindings::from_permissions(&perms(&[
            "context[ctx-1,id-1]",
            "context:execute",
        ]));
        assert_eq!(b.context_id.as_deref(), Some("ctx-1"));
        assert!(b.application_id.is_none());
        assert!(b.permits("ctx-1", "any-app"));
        assert!(!b.permits("ctx-2", "any-app"));
    }

    #[test]
    fn reads_the_application_binding() {
        let b = ClientKeyBindings::from_permissions(&perms(&[
            "context:execute",
            &application_binding("app-a"),
        ]));
        assert_eq!(b.application_id.as_deref(), Some("app-a"));
        assert!(b.permits("any-ctx", "app-a"));
        assert!(!b.permits("any-ctx", "app-b"));
    }

    #[test]
    fn both_bindings_must_hold() {
        let b = ClientKeyBindings::from_permissions(&perms(&[
            "context[ctx-1,id]",
            &application_binding("app-a"),
        ]));
        assert!(b.permits("ctx-1", "app-a"));
        assert!(!b.permits("ctx-1", "app-b"));
        assert!(!b.permits("ctx-2", "app-a"));
    }

    #[test]
    fn scoped_route_permissions_are_not_bindings() {
        let b = ClientKeyBindings::from_permissions(&perms(&[
            "context:execute[ctx-9]",
            "context:alias:delete:context[alias-name]",
            "application:list[app-z]",
        ]));
        assert!(b.is_unbound());
    }

    #[test]
    fn an_unbound_key_permits_everything_as_before() {
        let b = ClientKeyBindings::from_permissions(&perms(&["context:execute", "context:list"]));
        assert!(b.is_unbound());
        assert!(b.permits("ctx", "app"));
    }

    #[test]
    fn a_binding_grants_no_route() {
        use crate::auth::permissions::PermissionValidator;
        use axum::body::Body;
        use axum::http::Request;

        let validator = PermissionValidator::new();
        let req = Request::builder()
            .method("POST")
            .uri("/jsonrpc")
            .body(Body::empty())
            .unwrap();
        let required = validator.determine_required_permissions(&req);
        assert!(!validator.validate_permissions(&[application_binding("app-a")], &required));
    }
}
