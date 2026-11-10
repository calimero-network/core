//! What a person sees of a passkey outside the node: the name a password
//! manager lists it under, and the answer to a sign-in with one this cluster
//! does not know.
//!
//! Every Calimero Cloud cluster shares one rp-id (`calimero.network`), while
//! accounts and passkeys are each cluster's own. So a browser offers a
//! passkey made on one cluster on every other one, and every passkey would be
//! listed under the same name. The name states the cluster (and the app, for
//! a sign-up on a package's frontend), and a sign-in with a passkey from
//! elsewhere is answered [`PASSKEY_UNKNOWN_HERE`].

use crate::error::ApiError;
use crate::node::Node;

/// The most characters a passkey name takes of each of its parts (the
/// person's name, the app's, the cluster's). Authenticators may cut a long
/// name anywhere, so every part is kept short enough to be read whole.
pub(super) const MAX_PASSKEY_NAME_PART_CHARS: usize = 32;

/// The error a sign-in or step-up answers for a passkey no account here has.
pub(super) const PASSKEY_UNKNOWN_HERE: &str = "PASSKEY_UNKNOWN_HERE";

/// `401 PASSKEY_UNKNOWN_HERE`. It names neither where the passkey is from
/// nor whose it is: the node cannot know, and says only that it is not one
/// of this cluster's.
pub(super) fn unknown_here() -> ApiError {
    ApiError::new(
        401,
        PASSKEY_UNKNOWN_HERE,
        "this passkey belongs to another Calimero cluster or was removed from this one; \
         sign up here, or open the app from the link for the cluster your account is on",
    )
}

/// The `user.name` and `user.displayName` of a passkey made here: `alice ·
/// eu-shared-1 · 3f2a`, or `alice · kv-store (eu-shared-1 · 3f2a)` when it is
/// made on the frontend of the app named `app`. `tag` is [`cluster_tag`]: a
/// cluster made again under the same name (a new cluster key) gets another,
/// so a password manager still holding the old cluster's passkeys lists them
/// apart from this one's. An empty `tag` is left out.
pub(super) fn passkey_user_name(
    person: &str,
    cluster: &str,
    tag: &str,
    app: Option<&str>,
) -> String {
    let person = clip(person);
    let cluster = if tag.is_empty() {
        clip(cluster)
    } else {
        format!("{} · {tag}", clip(cluster))
    };
    match app {
        Some(app) => format!("{person} · {} ({cluster})", clip(app)),
        None => format!("{person} · {cluster}"),
    }
}

/// This cluster's tag in a passkey's name: the first 2 bytes of its identity
/// key (`Node::cluster_identity`, from K_c), as 4 hex characters. Stable for
/// the cluster's life and the same on every node; a cluster begun again under
/// the same name has another cluster key, so another tag. Public in
/// `GET /sign-up` (`passkeyTag`), so a sign-in can say which passkey to pick.
/// Two bytes tell a recreated cluster from the old one; they are not an
/// identifier, and clusters on one storage key (every in-process test
/// cluster runs on `[42; 32]`) share a tag.
pub(super) fn cluster_tag(node: &Node) -> String {
    hex::encode(&node.cluster_identity()[..2])
}

/// The name of `package` a person recognizes: its newest release's display
/// name, else the package id.
pub(super) fn app_name(node: &Node, package: &str) -> String {
    node.sys(|s| {
        s.packages
            .get(package)?
            .latest()?
            .metadata
            .get("name")?
            .as_str()
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
    })
    .unwrap_or_else(|| package.to_owned())
}

fn clip(s: &str) -> String {
    let s = s.trim();
    if s.chars().count() <= MAX_PASSKEY_NAME_PART_CHARS {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(MAX_PASSKEY_NAME_PART_CHARS - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::{passkey_user_name, unknown_here, MAX_PASSKEY_NAME_PART_CHARS};

    #[test]
    fn a_passkey_is_named_by_its_cluster_and_app() {
        assert_eq!(
            passkey_user_name("alice", "eu-shared-1", "3f2a", None),
            "alice · eu-shared-1 · 3f2a"
        );
        assert_eq!(
            passkey_user_name("alice", "eu-shared-1", "3f2a", Some("kv-store")),
            "alice · kv-store (eu-shared-1 · 3f2a)"
        );
        assert_eq!(
            passkey_user_name("alice", "eu-shared-1", "", None),
            "alice · eu-shared-1"
        );
    }

    #[test]
    fn each_part_of_a_passkey_name_is_bounded() {
        let long = "x".repeat(500);
        let name = passkey_user_name(&long, &long, "3f2a", Some(&long));
        assert!(
            name.chars().count() <= 3 * MAX_PASSKEY_NAME_PART_CHARS + 13,
            "{name}"
        );
        assert!(name.contains('…'), "{name}");
    }

    #[test]
    fn an_unknown_passkey_names_no_other_cluster_or_account() {
        let e = unknown_here();
        assert_eq!((e.status, e.code.as_str()), (401, "PASSKEY_UNKNOWN_HERE"));
        assert!(e.message.contains("another Calimero cluster"), "{e}");
        assert!(e.details.is_null(), "{e}");
    }
}
