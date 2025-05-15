use std::path::{Path, PathBuf};
use std::{env, fs};

use cached_path::{Cache, Options};
use reqwest::blocking::Client;
use serde::Deserialize;

#[derive(Deserialize)]
struct Release {
    tag_name: String,
}

const CALIMERO_WEB_UI_RELEASE: &str =
    "https://api.github.com/repos/calimero-network/admin-dashboard/releases/latest";
const CALIMERO_WEB_UI_SRC: &str =
    "https://github.com/calimero-network/admin-dashboard/archive/refs/tags/";

fn main() {
    let client = Client::new();
    let release: Release = client
        .get(CALIMERO_WEB_UI_RELEASE)
        .header("User-Agent", "rust-client")
        .send()
        .unwrap_or_else(|e| {
            eprintln!("Failed to send request: {e}");
            std::process::exit(1);
        })
        .json()
        .unwrap_or_else(|e| {
            eprintln!("Failed to parse JSON: {e}");
            std::process::exit(1);
        });

    let latest_release_src = format!("{}{}.zip", CALIMERO_WEB_UI_SRC, release.tag_name);
    let src = option_env!("CALIMERO_WEB_UI_SRC").unwrap_or(&latest_release_src);

    let cache = Cache::builder()
        .dir(PathBuf::from(
            env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into()),
        ))
        .build()
        .expect("Failed to create cache");

    let options = Options::default().extract();

    let extracted_dir = cache
        .cached_path_with_options(src, &options)
        .expect("Failed to fetch or cache UI archive");

    let extracted_folder = fs::read_dir(&extracted_dir)
        .unwrap()
        .filter_map(Result::ok)
        .find(|entry| entry.path().is_dir())
        .expect("No extracted directory found")
        .path();

    let renamed_path = extracted_dir.join("admin-dashboard");

    if extracted_folder != renamed_path {
        if renamed_path.exists() {
            fs::remove_dir_all(&renamed_path)
                .unwrap_or_else(|e| panic!("Failed to remove directory {:?}: {}", renamed_path, e));
        }

        if extracted_folder.exists() {
            fs::rename(&extracted_folder, &renamed_path).unwrap_or_else(|e| {
                panic!(
                    "Failed to rename {:?} to {:?}: {}",
                    extracted_folder, renamed_path, e
                )
            });
        } else {
            panic!(
                "Expected extracted folder {:?} does not exist. Archive structure may have changed.",
                extracted_folder
            );
        }
    } else {
        println!("Extracted folder already named 'admin-dashboard', skipping rename.");
    }

    let extracted_build_path = renamed_path.join("build");

    println!(
        "cargo:rustc-env=CALIMERO_WEB_UI_PATH={}",
        extracted_build_path.display()
    );

    if Path::new(src).exists() {
        println!("cargo:rerun-if-changed={}", src);
    }
}
