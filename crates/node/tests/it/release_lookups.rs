//! A route reads a release a client names only through `SystemState`'s lookups
//! (`release_used`, `releases_used`, `package_used`, `newest_used`, `newest_read`,
//! `release_offered_to`), which answer anyone the rule leaves out as if the release were not
//! installed. This gate fails on a direct read of the package table, or of the release a
//! bundle record names, under `src/api` or `src/consent_page` that `READS_ALLOWED` does not
//! name.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Direct reads of the package table, matched with a chain's lines joined.
const DIRECT_READS: &[&str] = &[
    "packages.get(",
    "packages.contains_key(",
    "packages.values(",
    "packages.iter(",
    ".versions.get(",
    ".versions.contains_key(",
    ".versions.values(",
    ".versions.iter(",
    ".versions.keys(",
    ".latest()",
    // A bundle record names the release its WASM was first recorded from, not the one an
    // instance runs (`report::release_of_app`).
    "bundles.get(",
    "b.package",
];

/// (file under `src/`, a fragment of exactly one joined line, why no client names the release).
#[rustfmt::skip]
const READS_ALLOWED: &[(&str, &str, &str)] = &[
    ("api/apps.rs", "s.packages.contains_key(pkg)", "a bound session's own package; accepted for now in the spec"),
    ("api/apps.rs", "s.packages.get(&r.package).and_then(|p| p.versions.get(&r.version)).cloned()", "instance_org_of: a release the create resolved"),
    ("api/apps.rs", "let v = s.packages.get(&r.package)?.versions.get(&r.version)?;", "chosen_parts: the member unit_members chose"),
    ("api/apps.rs", "s.packages.get(&release.package).and_then(|p| p.versions.get(&release.version))", "unit_members: a release its callers resolved"),
    ("api/apps.rs", "s.packages.get(&u.package).map(|p| {", "unit_members: the member rule itself"),
    ("api/apps.rs", "p.versions.iter().filter(|(_, v)| same_signer || s.sees_release(v, creator) || serves(v))", "unit_members: the member rule itself"),
    ("api/packages.rs", "s.packages.values().filter_map(|p| {", "package_list: package_json lists the releases used"),
    ("api/packages.rs", "s.packages.values().filter_map(|p| package_json(s, p, crate::system::OPERATOR, false))", "ops_package_list: the operator's"),
    ("api/passkeys.rs", "s.packages.get(package)?.latest_where(|v| {", "app_name: a name only from a release open to anyone"),
    ("api/upgrades.rs", "let v = s.packages.get(&to.package)?.versions.get(&to.version)?;", "auto_upgrade: the leader's own pass, judged by auto_update_allows"),
    ("api/upgrades.rs", "&& s.packages.get(&package).and_then(|pk| s.publisher_of_signer(&pk.signer_id))", "publisher_upgrade: the package's own publisher"),
    ("api/upgrades.rs", "s.packages.get(&release.at.package).and_then(|p| p.versions.get(&release.at.version))", "refuse_off_unit_range: the release the unit's instance runs"),
    ("consent_page.rs", "let v = s.packages.get(package)?.versions.get(version)?;", "requested_release: a unit member's target from unit_plan"),
    ("consent_page.rs", "node.sys(|s| s.packages.get(p)?.versions.get(v).cloned())", "target_manifest_hash: the target upgrade_app resolved"),
    ("consent_page.rs", "s.packages.get(&package)?.versions.get(&to_version).cloned()", "upgrade_request: the target requested_release resolved"),
    ("consent_page.rs", "s.packages.get(&install.package)?.versions.get(install.version.as_deref()?)", "still_current: a pending action the node built, by hash"),
    ("consent_page.rs", "s.packages.get(package).and_then(|p| p.versions.get(version)).map(|v| v.mpk.clone())", "still_current: a pending action the node built, by hash"),
    ("consent_page/updates.rs", "s.packages.get(&release.package)?.versions.get(&release.version).cloned()", "install_request: the release resolved above"),
    ("consent_page/updates.rs", "s.packages.get(&r.package).and_then(|p| p.versions.get(&r.version)).cloned()", "install_request: a member release_unit chose"),
];

/// Every `.rs` file under `dir`.
fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir)
        .expect("read a source directory")
        .flatten()
    {
        let path = entry.path();
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// `src`'s lines trimmed, a line that starts with `.` joined to the one before (a method
/// chain rustfmt split), each with the number of its first line.
fn joined(src: &str) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let line = line.trim();
        match out.last_mut() {
            Some((_, last)) if line.starts_with('.') => last.push_str(line),
            _ => out.push((i + 1, line.to_owned())),
        }
    }
    out
}

#[test]
fn routes_read_releases_through_the_rule() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = vec![root.join("api.rs"), root.join("consent_page.rs")];
    sources(&root.join("api"), &mut files);
    sources(&root.join("consent_page"), &mut files);
    let mut found = Vec::new();
    let mut matched = vec![BTreeSet::new(); READS_ALLOWED.len()];
    for path in files {
        let src = std::fs::read_to_string(&path).unwrap();
        let name = path
            .strip_prefix(&root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        for (n, line) in joined(&src) {
            if !DIRECT_READS.iter().any(|r| line.contains(r)) {
                continue;
            }
            match READS_ALLOWED
                .iter()
                .position(|(f, frag, _)| *f == name && line.contains(frag))
            {
                Some(i) => {
                    let _new = matched[i].insert(format!("{name}:{n}"));
                }
                None => found.push(format!("{name}:{n}: {line}")),
            }
        }
    }
    assert!(
        found.is_empty(),
        "read a release a client names through SystemState::release_used and its kin, or name \
         the line in READS_ALLOWED with why no client names it:\n{}",
        found.join("\n")
    );
    let off: Vec<String> = READS_ALLOWED
        .iter()
        .zip(&matched)
        .filter(|(_, at)| at.len() != 1)
        .map(|((f, frag, _), at)| format!("{f} `{frag}` matches {at:?}"))
        .collect();
    assert!(
        off.is_empty(),
        "each allowed read names exactly one line:\n{}",
        off.join("\n")
    );
}
