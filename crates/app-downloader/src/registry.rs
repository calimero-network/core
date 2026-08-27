//! The `[registry]` config and `package@version` coordinates.

use std::str::FromStr;

use eyre::OptionExt;
use serde::{Deserialize, Serialize};
use url::Url;

/// Marker for an application source with no fetchable location; join and upgrade read it as "peers only".
pub const PENDING_BLOB_SHARE_SOURCE: &str = "calimero://pending-blob-share";

const ARTIFACT_PREFIX: &str = "artifacts"; // `/api/artifacts/...` handler; `/artifacts/...` is the documented rewrite
const ARTIFACT_EXTENSION: &str = "mpk"; // bundles are the distribution unit
const PLACEHOLDER_COORDS: (&str, &str) = ("unknown", "0.0.0"); // stamped when raw-wasm install has no manifest; stored_coords must reject it by name
const MAX_COORD_LEN: usize = 128; // bounds a hostile op's URL

/// Which single source this node uses for application bundles. `Http` nodes
/// neither fetch from nor serve to peers; `Dht` nodes do both.
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RegistryMode {
    #[default]
    Http,
    Dht,
}

impl FromStr for RegistryMode {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "http" => Ok(Self::Http),
            "dht" => Ok(Self::Dht),
            other => Err(format!(
                "unknown registry mode {other:?}, expected \"http\" or \"dht\""
            )),
        }
    }
}

/// `[registry]` config. Absent means resolution is off and P2P blob share
/// stays the only transport - the behavior that predates this section.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[non_exhaustive]
pub struct RegistryConfig {
    #[serde(default)]
    pub mode: RegistryMode,
    /// Operator-configured and the only URL an application is ever fetched
    /// from; the coordinates appended to it are what gets validated.
    #[serde(default)]
    pub base_url: Option<Url>,
}

impl RegistryConfig {
    #[must_use]
    pub const fn new(mode: RegistryMode, base_url: Option<Url>) -> Self {
        Self { mode, base_url }
    }

    /// The base `Http` mode fetches from. There is no second route behind it,
    /// so a node configured without one cannot resolve applications at all.
    pub fn http_base(&self) -> eyre::Result<&Url> {
        self.base_url
            .as_ref()
            .ok_or_eyre("[registry] mode = \"http\" needs a base_url to fetch from")
    }
}

/// The coordinates of one published application version, as carried by a
/// governance op. Borrowed because every caller already owns the strings.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RegistryCoords<'a> {
    pub package: &'a str,
    pub version: &'a str,
}

/// Owned [`RegistryCoords`], for coordinates that must outlive the row they
/// were read from. Same relationship as `Path` to `PathBuf`.
#[derive(Clone, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct RegistryCoordsBuf {
    pub package: String,
    pub version: String,
}

impl RegistryCoordsBuf {
    #[must_use]
    pub fn new(package: String, version: String) -> Self {
        Self { package, version }
    }

    #[must_use]
    pub fn coords(&self) -> RegistryCoords<'_> {
        RegistryCoords::new(&self.package, &self.version)
    }
}

impl<'a> RegistryCoords<'a> {
    /// Both halves or neither: a lone package or version is not a location.
    #[must_use]
    pub const fn new(package: &'a str, version: &'a str) -> Self {
        Self { package, version }
    }

    #[must_use]
    pub fn to_buf(&self) -> RegistryCoordsBuf {
        RegistryCoordsBuf::new(self.package.to_owned(), self.version.to_owned())
    }

    /// Builds `{base}/artifacts/{package}/{version}/{package}-{version}.mpk`;
    /// repeats the coordinates in the filename to match the registry's own layout.
    #[must_use]
    pub fn artifact_url(&self, base: &Url) -> Option<Url> {
        if !is_safe_coord(self.package) || !is_safe_coord(self.version) {
            return None;
        }
        let mut url = base.clone();
        {
            // `path_segments_mut` fails only on a cannot-be-a-base URL (e.g.
            // `mailto:`), which is not a usable registry base either way.
            let mut segments = url.path_segments_mut().ok()?;
            // A base written with a trailing slash would otherwise leave an
            // empty segment in the middle of the path.
            segments.pop_if_empty();
            let _ = segments
                .push(ARTIFACT_PREFIX)
                .push(self.package)
                .push(self.version)
                .push(&format!(
                    "{}-{}.{ARTIFACT_EXTENSION}",
                    self.package, self.version
                ));
        }
        Some(url)
    }
}

/// Whether one coordinate is a safe, single path segment.
fn is_safe_coord(coord: &str) -> bool {
    if coord.is_empty() || coord.len() > MAX_COORD_LEN {
        return false;
    }
    if coord == "." || coord == ".." || coord.contains("..") {
        return false;
    }
    coord
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// The coordinates a stored application row carries, for the fetch paths that
/// read one back. The wire always names both halves, but a row is written
/// before its install completes and its version parses as semver or not at all,
/// so a row - unlike an op - can still be unaddressable.
#[must_use]
pub fn stored_coords<'a>(package: &'a str, version: &'a str) -> Option<RegistryCoords<'a>> {
    if package.is_empty() || version.is_empty() || (package, version) == PLACEHOLDER_COORDS {
        return None;
    }
    Some(RegistryCoords::new(package, version))
}

#[cfg(test)]
mod tests {
    use url::Url;

    use super::{stored_coords, RegistryConfig, RegistryCoords, RegistryMode};

    fn base() -> Url {
        "https://apps.calimero.network".parse().expect("valid base")
    }

    // The exact shape the registry serves. Verified against
    // `packages/frontend/src/lib/api.ts` in the app-registry repo, whose
    // artifact convention is `/artifacts/:package/:version/:package-:version.mpk`,
    // and against `ENV_CONFIG.md`, whose GCS objects live at
    // `{prefix}/{package}/{version}.mpk`.
    #[test]
    fn builds_the_artifact_url_from_coordinates() {
        let coords = RegistryCoords::new("com.calimero.migration-suite", "2.0.0");
        assert_eq!(
            coords.artifact_url(&base()).map(String::from),
            Some(
                "https://apps.calimero.network/artifacts/com.calimero.migration-suite/2.0.0/\
                 com.calimero.migration-suite-2.0.0.mpk"
                    .to_owned()
            )
        );
    }

    #[test]
    fn keeps_a_base_path_prefix_intact() {
        let base: Url = "https://reg.acme.internal/mero".parse().expect("valid");
        let coords = RegistryCoords::new("com.acme.app", "1.2.3");
        assert_eq!(
            coords.artifact_url(&base).map(String::from),
            Some(
                "https://reg.acme.internal/mero/artifacts/com.acme.app/1.2.3/\
                 com.acme.app-1.2.3.mpk"
                    .to_owned()
            )
        );
    }

    #[test]
    fn refuses_a_base_that_cannot_be_a_base() {
        let base: Url = "mailto:someone@example.com".parse().expect("valid");
        let coords = RegistryCoords::new("com.acme.app", "1.0.0");
        assert_eq!(coords.artifact_url(&base), None);
    }

    // A coordinate is a manifest field, never free text. Anything that could
    // walk out of the configured base, or smuggle a second path segment, must
    // yield no URL at all rather than a sanitized one.
    #[test]
    fn refuses_coordinates_that_could_escape_the_base() {
        for (package, version) in [
            ("..", "1.0.0"),
            ("a/../../etc", "1.0.0"),
            ("com.acme.app", ".."),
            ("com.acme.app", "1.0.0/../../secret"),
            ("com.acme/app", "1.0.0"),
            ("", "1.0.0"),
            ("com.acme.app", ""),
            ("com.acme.app", "1.0.0?x=y"),
            ("com.acme.app", "1.0.0#frag"),
            ("com.acme.app", "v ersion"),
            ("com.acme.app", "1.0.0%2f.."),
        ] {
            let coords = RegistryCoords::new(package, version);
            assert_eq!(
                coords.artifact_url(&base()),
                None,
                "package={package:?} version={version:?} must not produce a URL"
            );
        }
    }

    #[test]
    fn refuses_an_over_long_coordinate() {
        let long = "a".repeat(129);
        let coords = RegistryCoords::new(&long, "1.0.0");
        assert_eq!(coords.artifact_url(&base()), None);
    }

    #[test]
    fn accepts_a_coordinate_at_the_length_boundary() {
        let max = "a".repeat(128);
        let coords = RegistryCoords::new(&max, "1.0.0");
        assert!(coords.artifact_url(&base()).is_some());
    }

    #[test]
    fn accepts_the_full_permitted_charset() {
        let coords = RegistryCoords::new("com.acme_test-app.v2", "1.0.0-rc.1_build-9");
        assert!(coords.artifact_url(&base()).is_some());
    }

    #[test]
    fn stored_coords_need_both_halves() {
        assert_eq!(
            stored_coords("com.acme.app", "1.0.0"),
            Some(RegistryCoords::new("com.acme.app", "1.0.0"))
        );
        assert_eq!(stored_coords("", "1.0.0"), None);
        assert_eq!(stored_coords("com.acme.app", ""), None);
    }

    // The raw-wasm install path stamps this pair when it has no manifest to
    // read. Signing it onto an op would aim receivers at a URL nobody published.
    #[test]
    fn stored_coords_reject_the_raw_wasm_placeholder() {
        assert_eq!(stored_coords("unknown", "0.0.0"), None);
    }

    #[test]
    fn registry_defaults_to_disabled() {
        let cfg = RegistryConfig::default();
        assert!(
            cfg.base_url.is_none(),
            "absent [registry] must not change behavior"
        );
    }

    #[test]
    fn registry_section_deserializes_from_toml() {
        let cfg: RegistryConfig = toml::from_str(
            r#"
            base_url = "https://apps.calimero.network"
            "#,
        )
        .expect("[registry] section must deserialize");
        assert_eq!(
            cfg.base_url.map(String::from),
            Some("https://apps.calimero.network/".to_owned())
        );
    }

    #[test]
    fn empty_registry_section_deserializes_to_default() {
        let cfg: RegistryConfig = toml::from_str("").expect("empty section must deserialize");
        assert!(cfg.base_url.is_none());
    }

    #[test]
    fn mode_defaults_to_http() {
        let cfg: RegistryConfig = toml::from_str(
            r#"
            base_url = "https://apps.calimero.network"
            "#,
        )
        .expect("[registry] section without mode must deserialize");
        assert_eq!(cfg.mode, RegistryMode::Http);
    }

    #[test]
    fn mode_dht_parses() {
        let cfg: RegistryConfig =
            toml::from_str(r#"mode = "dht""#).expect("mode = dht must deserialize");
        assert_eq!(cfg.mode, RegistryMode::Dht);
        assert_eq!("dht".parse::<RegistryMode>(), Ok(RegistryMode::Dht));
    }

    #[test]
    fn unknown_mode_is_an_error() {
        assert!(toml::from_str::<RegistryConfig>(r#"mode = "carrier-pigeon""#).is_err());
        assert!("carrier-pigeon".parse::<RegistryMode>().is_err());
    }
}
