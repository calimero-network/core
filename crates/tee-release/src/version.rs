//! Release version strings: validation, tag prefixes and ordering.

use std::cmp::Ordering;

use eyre::{bail, Result as EyreResult};

/// Normalise a release version, accepting it bare (`2.3.71`) or behind a tag
/// prefix (`mero-kms-v2.3.71` with `tag_prefix = "mero-kms-v"`).
///
/// The result is interpolated into a download URL, so anything that is not
/// semver-shaped is refused rather than escaped.
pub fn normalize_release_version(raw: &str, tag_prefix: &str) -> EyreResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        bail!("release version cannot be empty");
    }

    let version = trimmed.strip_prefix(tag_prefix).unwrap_or(trimmed);
    if !is_valid_release_version(version) {
        bail!(
            "release version must be semver-like (e.g. 2.1.14 or 2.1.14-rc.1), got '{}'",
            trimmed
        );
    }

    Ok(version.to_owned())
}

/// Whether `version` is `MAJOR.MINOR.PATCH` with an optional `-pre`/`+build`
/// suffix of URL-safe characters.
pub fn is_valid_release_version(version: &str) -> bool {
    let mut core_and_suffix = version.splitn(2, ['-', '+']);
    let core = core_and_suffix.next().unwrap_or_default();
    let suffix = core_and_suffix.next();

    let mut core_segments = core.split('.');
    let (Some(major), Some(minor), Some(patch)) = (
        core_segments.next(),
        core_segments.next(),
        core_segments.next(),
    ) else {
        return false;
    };
    if core_segments.next().is_some() {
        return false;
    }
    for segment in [major, minor, patch] {
        if segment.is_empty() || !segment.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }

    if let Some(suffix) = suffix {
        if suffix.is_empty()
            || !suffix
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '+')
        {
            return false;
        }
    }

    true
}

/// Order two valid release versions by semver precedence.
///
/// `MAJOR.MINOR.PATCH` compares numerically, a pre-release sorts before its
/// release (`2.3.72-rc.1 < 2.3.72`), pre-release identifiers compare
/// numerically when both are numbers, and build metadata is ignored. Returns
/// `None` when either side is not a valid version, so a caller cannot mistake
/// "cannot compare" for "older".
pub fn compare_release_versions(a: &str, b: &str) -> Option<Ordering> {
    if !is_valid_release_version(a) || !is_valid_release_version(b) {
        return None;
    }
    let (a_core, a_pre) = split_version(a);
    let (b_core, b_pre) = split_version(b);
    let core = a_core.cmp(&b_core);
    if core != Ordering::Equal {
        return Some(core);
    }
    Some(match (a_pre, b_pre) {
        (None, None) => Ordering::Equal,
        (None, Some(_)) => Ordering::Greater,
        (Some(_), None) => Ordering::Less,
        (Some(a), Some(b)) => compare_pre_release(a, b),
    })
}

fn split_version(version: &str) -> ([u64; 3], Option<&str>) {
    let without_build = version.split('+').next().unwrap_or(version);
    let mut parts = without_build.splitn(2, '-');
    let core = parts.next().unwrap_or_default();
    let pre = parts.next();
    let mut nums = [0u64; 3];
    for (slot, segment) in nums.iter_mut().zip(core.split('.')) {
        *slot = segment.parse().unwrap_or(u64::MAX);
    }
    (nums, pre)
}

fn compare_pre_release(a: &str, b: &str) -> Ordering {
    let mut a_ids = a.split('.');
    let mut b_ids = b.split('.');
    loop {
        match (a_ids.next(), b_ids.next()) {
            (None, None) => return Ordering::Equal,
            (None, Some(_)) => return Ordering::Less,
            (Some(_), None) => return Ordering::Greater,
            (Some(x), Some(y)) => {
                let ord = match (x.parse::<u64>(), y.parse::<u64>()) {
                    (Ok(x), Ok(y)) => x.cmp(&y),
                    (Ok(_), Err(_)) => Ordering::Less,
                    (Err(_), Ok(_)) => Ordering::Greater,
                    (Err(_), Err(_)) => x.cmp(y),
                };
                if ord != Ordering::Equal {
                    return ord;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_prefixed_tag_normalises_to_its_version() {
        assert_eq!(
            normalize_release_version("mero-kms-v2.1.14", "mero-kms-v").unwrap(),
            "2.1.14"
        );
        assert_eq!(
            normalize_release_version(" 2.3.71 ", "mero-tee-v").unwrap(),
            "2.3.71"
        );
    }

    #[test]
    fn a_version_that_could_escape_the_url_is_refused() {
        for bad in ["../evil", "2.3", "2.3.x", "2.3.71/", "", "2.3.71-"] {
            assert!(
                normalize_release_version(bad, "mero-tee-v").is_err(),
                "{bad}"
            );
        }
    }

    #[test]
    fn versions_order_by_semver_not_by_string() {
        assert_eq!(
            compare_release_versions("2.3.9", "2.3.68"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_release_versions("2.3.72-rc.1", "2.3.72"),
            Some(Ordering::Less)
        );
        assert_eq!(
            compare_release_versions("2.3.72-rc.10", "2.3.72-rc.9"),
            Some(Ordering::Greater)
        );
        assert_eq!(
            compare_release_versions("2.3.72+build.1", "2.3.72"),
            Some(Ordering::Equal)
        );
        assert_eq!(compare_release_versions("nope", "2.3.72"), None);
    }
}
