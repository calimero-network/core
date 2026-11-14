//! `meroctl dev new` against a local stand-in for the apps repository: no
//! network, no node.

use std::path::Path;
use std::process::Command;

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .current_dir(dir)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@t",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .status()
        .unwrap()
        .success();
    assert!(ok, "git {args:?}");
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

/// The apps repository's layout: root files, two apps, shared packages and scripts.
fn apps_repo(root: &Path) -> String {
    for (rel, text) in [
        ("package.json", "{}"),
        (
            "pnpm-workspace.yaml",
            "packages: [\"apps/*/app\", \"packages/*\"]",
        ),
        ("Cargo.toml", "[workspace]"),
        ("README.md", "the monorepo"),
        ("apps/kv-store/logic/Cargo.toml", "[package]"),
        ("apps/kv-store/app/package.json", "{}"),
        ("apps/mero-chat/logic/Cargo.toml", "[package]"),
        ("packages/connect/package.json", "{}"),
        ("scripts/component-build.sh", "#!/bin/sh"),
        ("docs/COMPONENT-APPS.md", "docs"),
    ] {
        write(root, rel, text);
    }
    git(root, &["init", "--quiet"]);
    git(root, &["add", "-A"]);
    git(root, &["commit", "--quiet", "-m", "apps"]);
    format!("file://{}", root.display())
}

/// `meroctl` with a git identity of its own, whatever the machine has (CI has
/// none): `dev new` commits what the template gave.
fn meroctl(cwd: &Path, args: &[&str]) -> std::process::Output {
    meroctl_with(
        cwd,
        args,
        &[
            ("GIT_AUTHOR_NAME", "t"),
            ("GIT_AUTHOR_EMAIL", "t@t"),
            ("GIT_COMMITTER_NAME", "t"),
            ("GIT_COMMITTER_EMAIL", "t@t"),
        ],
    )
}

fn meroctl_with(cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_meroctl"))
        .current_dir(cwd)
        .env("CALIMERO_HOME", cwd.join("home"))
        .envs(env.iter().copied())
        .args(args)
        .output()
        .unwrap()
}

#[test]
fn dev_new_starts_a_project_from_an_app_of_the_apps_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = apps_repo(&tmp.path().join("apps-repo"));

    let out = meroctl(tmp.path(), &["dev", "new", "my-app", "--repo", &repo]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let answer: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    assert_eq!(answer["app"], "apps/kv-store");

    let p = tmp.path().join("my-app");
    // The app, what it shares, and the root files that pin its versions.
    for kept in [
        "apps/kv-store/logic/Cargo.toml",
        "apps/kv-store/app/package.json",
        "packages/connect/package.json",
        "scripts/component-build.sh",
        "pnpm-workspace.yaml",
        "Cargo.toml",
    ] {
        assert!(p.join(kept).is_file(), "{kept} missing");
    }
    // No other app, nothing outside the cone, and not the monorepo's history.
    assert!(!p.join("apps/mero-chat").exists());
    assert!(!p.join("docs").exists());
    let readme = std::fs::read_to_string(p.join("README.md")).unwrap();
    assert!(readme.starts_with("# kv-store"), "{readme}");
    // One commit holds what the template gave, so the project's own changes
    // show as changes, and it names where it came from.
    let log = Command::new("git")
        .current_dir(&p)
        .args(["log", "--format=%s"])
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&log.stdout).into_owned();
    assert_eq!(log.lines().count(), 1, "{log}");
    assert!(log.contains("apps/kv-store"), "{log}");
    let status = Command::new("git")
        .current_dir(&p)
        .args(["status", "--porcelain"])
        .output()
        .unwrap();
    assert!(
        status.stdout.is_empty(),
        "{}",
        String::from_utf8_lossy(&status.stdout)
    );

    // A directory with something in it is refused, and left as it was.
    let again = meroctl(tmp.path(), &["dev", "new", "my-app", "--repo", &repo]);
    assert!(!again.status.success());
    assert!(String::from_utf8_lossy(&again.stderr).contains("not empty"));
    assert!(p.join("apps/kv-store").exists());

    // An app the repository does not have is refused, and leaves nothing behind.
    let missing = meroctl(
        tmp.path(),
        &["dev", "new", "other", "--repo", &repo, "--template", "nope"],
    );
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("has no app apps/nope"));
    assert!(!tmp.path().join("other").exists());
}

// With no git identity there is no one to commit as: the project is still made,
// its files left untracked, and `dev new` says how to get the commit.
#[test]
fn dev_new_without_a_git_identity_makes_the_project_and_says_so() {
    let tmp = tempfile::tempdir().unwrap();
    let repo = apps_repo(&tmp.path().join("apps-repo"));
    let empty = tmp.path().join("no-git-home");
    std::fs::create_dir_all(&empty).unwrap();
    let empty = empty.to_str().unwrap();

    let out = meroctl_with(
        tmp.path(),
        &["dev", "new", "my-app", "--repo", &repo],
        &[
            ("HOME", empty),
            ("XDG_CONFIG_HOME", empty),
            ("GIT_CONFIG_GLOBAL", "/dev/null"),
            ("GIT_CONFIG_NOSYSTEM", "1"),
        ],
    );
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("no first commit"),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let p = tmp.path().join("my-app");
    assert!(p.join("apps/kv-store/logic/Cargo.toml").is_file());
    let log = Command::new("git")
        .current_dir(&p)
        .args(["log", "--oneline"])
        .output()
        .unwrap();
    assert!(!log.status.success(), "no commit without an identity");
}
