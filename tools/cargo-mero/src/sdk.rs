//! Resolving the `calimero-sdk` dependency to the node release a bundle was
//! compiled against, for the manifest's `buildInfo` block.
//!
//! This answers a question `minRuntimeVersion` does not. That field is declared
//! by hand in `[package.metadata.calimero]`, defaults to `0.1.0`, and states a
//! floor the app refuses to run below. What gets read off here is derived from
//! the resolve graph the build actually used, so it cannot drift from the
//! bytecode it ships beside.

use cargo_metadata::Metadata;

use crate::manifest::SdkResolution;

/// The crate whose resolution answers "which node was this built against".
/// `calimero-sdk` is the app-facing entry point of core's workspace and shares
/// its version, so its resolution *is* the node release.
const SDK_CRATE: &str = "calimero-sdk";

/// Core's in-git `[workspace.package] version`, which cargo-workspaces rewrites
/// only when publishing to crates.io.
///
/// ⚠️ THE WHOLE REASON THIS MODULE PARSES SOURCE STRINGS. Every app in the fleet
/// depends on `calimero-sdk` by git tag, and cargo reports the crate's own
/// version for such a dependency — which is this placeholder, not the tag. A
/// checkout of `0.11.0-rc.34` reports `version = "0.0.0"`, so reading the
/// version field would have stamped `0.0.0` onto every bundle ever built.
const PLACEHOLDER_VERSION: &str = "0.0.0";

/// Resolve the SDK the build is compiled against, or `None` when the app does
/// not depend on `calimero-sdk` at all (in which case there is nothing truthful
/// to stamp, and the manifest simply omits `buildInfo`).
pub fn resolve(metadata: &Metadata) -> Option<SdkResolution> {
    let package = metadata
        .packages
        .iter()
        .find(|p| p.name.as_str() == SDK_CRATE)?;

    Some(classify(
        package.source.as_ref().map(|s| s.repr.as_str()),
        package.version.to_string().as_str(),
    ))
}

/// Pure core of `resolve`, so every source shape is testable without building
/// a real cargo workspace for each.
///
/// `repr` is cargo's source string; `None` means a path dependency, which cargo
/// leaves sourceless.
fn classify(repr: Option<&str>, package_version: &str) -> SdkResolution {
    let Some(repr) = repr else {
        // A path dependency: a local core checkout, which carries the in-git
        // placeholder version. Record where it came from and nothing more.
        return SdkResolution {
            source: "path".to_owned(),
            version: release_version(package_version),
            rev: None,
        };
    };

    if let Some(rest) = repr.strip_prefix("git+") {
        let (base, rev) = split_once_owned(rest, '#');
        let (_url, query) = split_once_owned(&base, '?');
        return SdkResolution {
            source: "git".to_owned(),
            // Only a tag names a release. A `branch=`/`rev=` dependency, or a
            // bare one on the default branch, names none — and the crate's own
            // version is the placeholder, so there is nothing to fall back to.
            // `rev` still identifies such a build exactly.
            version: query.as_deref().and_then(|q| query_value(q, "tag")),
            rev,
        };
    }

    // `registry+`/`sparse+`: a published crate, whose version is the real one.
    SdkResolution {
        source: "registry".to_owned(),
        version: release_version(package_version),
        rev: None,
    }
}

/// A version, unless it is the placeholder — which is a statement that the
/// version is unknown, and must not be published as though it were a release.
fn release_version(version: &str) -> Option<String> {
    (version != PLACEHOLDER_VERSION).then(|| version.to_owned())
}

/// `haystack` split at the first `sep`; the tail is `None` when absent, and
/// empty tails collapse to `None` so `"...#"` does not yield an empty rev.
fn split_once_owned(haystack: &str, sep: char) -> (String, Option<String>) {
    match haystack.split_once(sep) {
        Some((head, tail)) if !tail.is_empty() => (head.to_owned(), Some(tail.to_owned())),
        Some((head, _)) => (head.to_owned(), None),
        None => (haystack.to_owned(), None),
    }
}

/// The value of `key` in a cargo source query string (`tag=x&branch=y`).
fn query_value(query: &str, key: &str) -> Option<String> {
    query.split('&').find_map(|pair| {
        let (k, v) = pair.split_once('=')?;
        (k == key && !v.is_empty()).then(|| v.to_owned())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shape every app in the `apps` monorepo builds with, and the one the
    /// placeholder trap applies to: the tag is the answer, `0.0.0` is not.
    #[test]
    fn git_tag_dependency_reports_the_tag_not_the_placeholder_version() {
        let resolved = classify(
            Some(
                "git+https://github.com/calimero-network/core.git?tag=0.11.0-rc.34\
                 #6c6fb4ab4fe02500ab1262c643f52dcc6d6278bf",
            ),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.source, "git");
        assert_eq!(resolved.version.as_deref(), Some("0.11.0-rc.34"));
        assert_eq!(
            resolved.rev.as_deref(),
            Some("6c6fb4ab4fe02500ab1262c643f52dcc6d6278bf")
        );
    }

    /// A branch dependency names no release. Reporting the branch name as a
    /// version would put "master" in a version field; reporting the crate's own
    /// version would put `0.0.0` there. The commit is the honest answer.
    #[test]
    fn git_branch_dependency_reports_only_the_commit() {
        let resolved = classify(
            Some("git+https://github.com/calimero-network/core.git?branch=master#abc123"),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.source, "git");
        assert_eq!(resolved.version, None);
        assert_eq!(resolved.rev.as_deref(), Some("abc123"));
    }

    #[test]
    fn git_rev_dependency_reports_only_the_commit() {
        let resolved = classify(
            Some("git+https://github.com/calimero-network/core.git?rev=abc123#abc123"),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.version, None);
        assert_eq!(resolved.rev.as_deref(), Some("abc123"));
    }

    /// No query string at all: a dependency on the default branch.
    #[test]
    fn git_default_branch_dependency_reports_only_the_commit() {
        let resolved = classify(
            Some("git+https://github.com/calimero-network/core.git#abc123"),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.source, "git");
        assert_eq!(resolved.version, None);
        assert_eq!(resolved.rev.as_deref(), Some("abc123"));
    }

    /// A tag alongside another key must still be found, wherever it sits.
    #[test]
    fn tag_is_found_among_several_query_keys() {
        let resolved = classify(
            Some("git+https://example.com/core.git?branch=master&tag=0.11.0-rc.34#abc"),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.version.as_deref(), Some("0.11.0-rc.34"));
    }

    #[test]
    fn registry_dependency_reports_the_crate_version() {
        let resolved = classify(
            Some("registry+https://github.com/rust-lang/crates.io-index"),
            "0.11.0",
        );

        assert_eq!(resolved.source, "registry");
        assert_eq!(resolved.version.as_deref(), Some("0.11.0"));
        assert_eq!(resolved.rev, None);
    }

    #[test]
    fn sparse_registry_dependency_is_still_a_registry() {
        let resolved = classify(Some("sparse+https://index.crates.io/"), "0.11.0");

        assert_eq!(resolved.source, "registry");
        assert_eq!(resolved.version.as_deref(), Some("0.11.0"));
    }

    /// A local core checkout. Cargo gives a path dependency no source at all,
    /// and its version is the in-git placeholder — so this stamps provenance
    /// without inventing a release number.
    #[test]
    fn path_dependency_reports_no_version() {
        let resolved = classify(None, PLACEHOLDER_VERSION);

        assert_eq!(resolved.source, "path");
        assert_eq!(resolved.version, None);
        assert_eq!(resolved.rev, None);
    }

    /// The placeholder guard is on the value, not on the source kind: a
    /// registry dependency somehow reporting `0.0.0` is just as unpublishable.
    #[test]
    fn placeholder_version_is_never_reported_as_a_release() {
        let resolved = classify(
            Some("registry+https://github.com/rust-lang/crates.io-index"),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.version, None);
    }

    /// An empty fragment must not become an empty-string rev, which would
    /// serialize as `"sdkRev": ""` and read as a real but blank commit.
    #[test]
    fn empty_fragment_yields_no_rev() {
        let resolved = classify(
            Some("git+https://example.com/core.git?tag=1.0.0#"),
            PLACEHOLDER_VERSION,
        );

        assert_eq!(resolved.version.as_deref(), Some("1.0.0"));
        assert_eq!(resolved.rev, None);
    }
}
