//! A node-image release's signed measurements (`published-mrtds.json`).

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, OnceLock};

use eyre::{bail, Result as EyreResult};
use serde::Deserialize;
use tokio::sync::Mutex;

use crate::fetch::fetch_verified_asset;
use crate::sigstore_verify::NODE_RELEASE_IDENTITY;
use crate::version::normalize_release_version;

/// The asset a node release publishes its measurements in.
pub const PUBLISHED_MRTDS_ASSET: &str = "published-mrtds.json";

/// A node release tag is `mero-tee-v<version>`.
pub const NODE_RELEASE_TAG_PREFIX: &str = "mero-tee-v";

/// The measurements one image profile of a release is admitted on.
///
/// Values are lowercase hex with no `0x`, the form a verified quote's fields
/// are rendered in, so they compare with `==`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileMeasurements {
    pub allowed_mrtd: Vec<String>,
    pub allowed_rtmr0: Vec<String>,
    pub allowed_rtmr1: Vec<String>,
    pub allowed_rtmr2: Vec<String>,
    pub allowed_rtmr3: Vec<String>,
}

/// A node release's verified measurements, by profile name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRelease {
    pub version: String,
    pub profiles: BTreeMap<String, ProfileMeasurements>,
}

#[derive(Deserialize)]
struct PublishedMrtds {
    tag: String,
    profiles: BTreeMap<String, PublishedProfile>,
}

#[derive(Deserialize)]
struct PublishedProfile {
    #[serde(default)]
    mrtd: Option<String>,
    #[serde(default)]
    rtmr0: Option<String>,
    #[serde(default)]
    rtmr1: Option<String>,
    #[serde(default)]
    rtmr2: Option<String>,
    #[serde(default)]
    rtmr3: Option<String>,
    #[serde(default)]
    allowed_mrtd: Vec<String>,
    #[serde(default)]
    allowed_rtmr0: Vec<String>,
    #[serde(default)]
    allowed_rtmr1: Vec<String>,
    #[serde(default)]
    allowed_rtmr2: Vec<String>,
    #[serde(default)]
    allowed_rtmr3: Vec<String>,
}

impl NodeRelease {
    /// Parse a `published-mrtds.json` that claims to be release `version`.
    ///
    /// The file's own `tag` must name `version`: the signature proves who
    /// published the bytes, and this proves they are the bytes for the
    /// release the TEE says it runs, not another release's served under this
    /// tag. A profile that pins no MRTD, RTMR1, RTMR2 or RTMR3 is dropped
    /// rather than admitted on fewer registers, for the reason core refuses
    /// such a list policy: RTMR3 only names the image when the kernel and
    /// initrd before it are pinned too.
    pub fn from_published_mrtds(json: &str, version: &str) -> EyreResult<Self> {
        let parsed: PublishedMrtds = serde_json::from_str(json)
            .map_err(|e| eyre::eyre!("published-mrtds.json does not parse: {e}"))?;
        let tag = parsed.tag.trim();
        let tag = tag.strip_prefix(NODE_RELEASE_TAG_PREFIX).unwrap_or(tag);
        if tag != version {
            bail!("published-mrtds.json is for release '{tag}', not '{version}'");
        }

        let mut profiles = BTreeMap::new();
        for (name, profile) in parsed.profiles {
            let pick = |list: Vec<String>, single: Option<String>| -> EyreResult<Vec<String>> {
                let raw = if list.is_empty() {
                    single.into_iter().collect()
                } else {
                    list
                };
                raw.iter().map(|v| normalize_measurement(v)).collect()
            };
            let measurements = ProfileMeasurements {
                allowed_mrtd: pick(profile.allowed_mrtd, profile.mrtd)?,
                allowed_rtmr0: pick(profile.allowed_rtmr0, profile.rtmr0)?,
                allowed_rtmr1: pick(profile.allowed_rtmr1, profile.rtmr1)?,
                allowed_rtmr2: pick(profile.allowed_rtmr2, profile.rtmr2)?,
                allowed_rtmr3: pick(profile.allowed_rtmr3, profile.rtmr3)?,
            };
            if measurements.allowed_mrtd.is_empty()
                || measurements.allowed_rtmr1.is_empty()
                || measurements.allowed_rtmr2.is_empty()
                || measurements.allowed_rtmr3.is_empty()
            {
                tracing::warn!(
                    release = version,
                    profile = %name,
                    "release profile pins no MRTD, RTMR1, RTMR2 or RTMR3; not admitting on it"
                );
                continue;
            }
            let _ = profiles.insert(name, measurements);
        }
        Ok(Self {
            version: version.to_owned(),
            profiles,
        })
    }

    /// The profile of `allowed_profiles` whose measurements the quote's
    /// registers match, if any. RTMR0 is checked only when the profile pins
    /// it: it varies with the VM's shape, not the image.
    pub fn matching_profile<'a>(
        &self,
        allowed_profiles: &'a [String],
        mrtd: &str,
        rtmr0: &str,
        rtmr1: &str,
        rtmr2: &str,
        rtmr3: &str,
    ) -> Option<&'a str> {
        let has = |list: &[String], value: &str| list.iter().any(|v| v == value);
        allowed_profiles.iter().map(String::as_str).find(|name| {
            self.profiles.get(*name).is_some_and(|p| {
                has(&p.allowed_mrtd, mrtd)
                    && (p.allowed_rtmr0.is_empty() || has(&p.allowed_rtmr0, rtmr0))
                    && has(&p.allowed_rtmr1, rtmr1)
                    && has(&p.allowed_rtmr2, rtmr2)
                    && has(&p.allowed_rtmr3, rtmr3)
            })
        })
    }
}

fn normalize_measurement(value: &str) -> EyreResult<String> {
    let v = value.trim().to_ascii_lowercase();
    let v = v.strip_prefix("0x").unwrap_or(&v).to_owned();
    if v.len() != 96 || !v.chars().all(|c| c.is_ascii_hexdigit()) {
        bail!("measurement '{value}' is not 48 bytes of hex");
    }
    Ok(v)
}

/// Releases never change once published, so a verified one is kept for the
/// life of the process. Bounded, because the version comes from whoever asks
/// to be admitted.
const CACHE_CAPACITY: usize = 32;

fn cache() -> &'static Mutex<HashMap<String, Arc<NodeRelease>>> {
    static CACHE: OnceLock<Mutex<HashMap<String, Arc<NodeRelease>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fetch and verify release `version`'s `published-mrtds.json`.
///
/// Only a verified release is cached: a failed fetch is tried again on the
/// next admission rather than remembered, since GitHub being unreachable for a
/// moment is not a property of the release.
pub async fn fetch_node_release(version: &str) -> EyreResult<Arc<NodeRelease>> {
    let version = normalize_release_version(version, NODE_RELEASE_TAG_PREFIX)?;
    if let Some(hit) = cache().lock().await.get(&version) {
        return Ok(Arc::clone(hit));
    }

    let tag = format!("{NODE_RELEASE_TAG_PREFIX}{version}");
    let body = fetch_verified_asset(&tag, PUBLISHED_MRTDS_ASSET, &NODE_RELEASE_IDENTITY).await?;
    let release = Arc::new(NodeRelease::from_published_mrtds(&body, &version)?);

    let mut cache = cache().lock().await;
    if cache.len() >= CACHE_CAPACITY {
        cache.clear();
    }
    let _ = cache.insert(version, Arc::clone(&release));
    Ok(release)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(c: char) -> String {
        std::iter::repeat_n(c, 96).collect()
    }

    fn published(tag: &str) -> String {
        serde_json::json!({
            "role": "node",
            "tag": tag,
            "profiles": {
                "locked-read-only": {
                    "mrtd": hex('a'), "rtmr0": hex('0'), "rtmr1": hex('1'), "rtmr2": hex('2'), "rtmr3": hex('3'),
                    "allowed_mrtd": [hex('a').to_uppercase()], "allowed_rtmr0": [], "allowed_rtmr1": [hex('1')],
                    "allowed_rtmr2": [hex('2')], "allowed_rtmr3": [format!("0x{}", hex('3'))]
                },
                "debug": {
                    "mrtd": hex('a'), "rtmr1": hex('1'), "rtmr2": hex('2'), "rtmr3": hex('d')
                },
                "no-rtmr3": { "mrtd": hex('a'), "rtmr1": hex('1'), "rtmr2": hex('2') }
            }
        })
        .to_string()
    }

    #[test]
    fn profiles_parse_to_normalised_lists_and_unpinned_ones_are_dropped() {
        let release = NodeRelease::from_published_mrtds(&published("2.3.72"), "2.3.72").unwrap();
        let locked = &release.profiles["locked-read-only"];
        assert_eq!(locked.allowed_mrtd, vec![hex('a')], "uppercase is lowered");
        assert_eq!(locked.allowed_rtmr3, vec![hex('3')], "0x is stripped");
        assert_eq!(
            locked.allowed_rtmr0,
            vec![hex('0')],
            "an empty list falls back to the single value"
        );
        assert_eq!(release.profiles["debug"].allowed_rtmr3, vec![hex('d')]);
        assert!(
            !release.profiles.contains_key("no-rtmr3"),
            "a profile without RTMR3 is not admitted on"
        );
    }

    #[test]
    fn a_file_for_another_release_is_refused() {
        let err = NodeRelease::from_published_mrtds(&published("2.3.71"), "2.3.72").unwrap_err();
        assert!(err.to_string().contains("2.3.71"), "{err}");
        assert!(
            NodeRelease::from_published_mrtds(&published("mero-tee-v2.3.72"), "2.3.72").is_ok()
        );
    }

    #[test]
    fn a_quote_matches_only_an_allowed_profile() {
        let release = NodeRelease::from_published_mrtds(&published("2.3.72"), "2.3.72").unwrap();
        let locked = vec!["locked-read-only".to_owned()];
        let (a, z, one, two, three) = (hex('a'), hex('0'), hex('1'), hex('2'), hex('3'));
        assert_eq!(
            release.matching_profile(&locked, &a, &z, &one, &two, &three),
            Some("locked-read-only")
        );
        // The debug image's RTMR3 matches the release, but debug is not allowed.
        assert_eq!(
            release.matching_profile(&locked, &a, &z, &one, &two, &hex('d')),
            None
        );
        let both = vec!["locked-read-only".to_owned(), "debug".to_owned()];
        assert_eq!(
            release.matching_profile(&both, &a, &z, &one, &two, &hex('d')),
            Some("debug")
        );
        // A different kernel (RTMR1) fails even with the right RTMR3.
        assert_eq!(
            release.matching_profile(&locked, &a, &z, &hex('9'), &two, &three),
            None
        );
    }

    #[test]
    fn a_measurement_that_is_not_48_bytes_fails_the_parse() {
        let json = serde_json::json!({
            "tag": "2.3.72",
            "profiles": { "locked-read-only": { "mrtd": "abcd", "rtmr1": hex('1'), "rtmr2": hex('2'), "rtmr3": hex('3') } }
        })
        .to_string();
        assert!(NodeRelease::from_published_mrtds(&json, "2.3.72").is_err());
    }
}
