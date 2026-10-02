pub const APPLICATION_BINDING: &str = "application-binding";

#[must_use]
pub fn application_binding(application_id: &str) -> String {
    format!("{APPLICATION_BINDING}[{application_id}]")
}

#[must_use]
pub fn is_application_binding(permission: &str) -> bool {
    bracketed(permission, APPLICATION_BINDING).is_some()
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClientKeyBindings {
    pub context_id: Option<String>,
    pub application_id: Option<String>,
}

impl ClientKeyBindings {
    #[must_use]
    pub fn from_permissions(permissions: &[String]) -> Self {
        let mut bindings = Self::default();

        for permission in permissions {
            if let Some(inner) = bracketed(permission, "context") {
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

    #[must_use]
    pub const fn is_unbound(&self) -> bool {
        self.context_id.is_none() && self.application_id.is_none()
    }

    #[must_use]
    pub fn permits(&self, context_id: &str, application_id: &str) -> bool {
        self.context_id
            .as_deref()
            .is_none_or(|c| c.eq_ignore_ascii_case(context_id))
            && self
                .application_id
                .as_deref()
                .is_none_or(|a| a.eq_ignore_ascii_case(application_id))
    }
}

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
    fn ids_compare_ignoring_case() {
        let b = ClientKeyBindings::from_permissions(&perms(&[
            "context[ABCDEF01,id]",
            &application_binding("DEADBEEF"),
        ]));
        assert!(b.permits("abcdef01", "deadbeef"));
        assert!(!b.permits("abcdef02", "deadbeef"));
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
    fn recognises_only_the_application_marker() {
        assert!(is_application_binding(&application_binding("app-a")));
        assert!(!is_application_binding("context[ctx,id]"));
        assert!(!is_application_binding("application:list[app-a]"));
        assert!(!is_application_binding("admin"));
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
