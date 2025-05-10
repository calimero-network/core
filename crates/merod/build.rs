use std::process::Command;

fn main() {
    println!("cargo:warning=Running build.rs");
    let output = Command::new("git")
        .args(["rev-parse", "--short=HEAD"])
        .output()
        .expect("Failed to get Git commit hash");

    let commit_hash = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let output = Command::new("git")
        .args(["describe", "--tags", "--dirty", "--always"])
        .output()
        .expect("Failed to get Git describe");

    let describe = String::from_utf8_lossy(&output.stdout).trim().to_string();

    let rustc_version = rustc_version::version().unwrap().to_string();

    println!("cargo:rustc-env=GIT_COMMIT_HASH={}", commit_hash);
    println!("cargo:rustc-env=GIT_DESCRIBE={}", describe);
    println!("cargo:rustc-env=RUSTC_VERSION={}", rustc_version);
}
