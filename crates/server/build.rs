use std::path::{Path, PathBuf};
use std::env;

use cached_path::{Cache, Options};

const CALIMERO_WEB_UI_SRC: &str =
    "https://github.com/calimero-network/admin-dashboard/archive/refs/heads/master.zip";

fn main() {
    let src = option_env!("CALIMERO_WEB_UI_SRC").unwrap_or(CALIMERO_WEB_UI_SRC);

    let cache = Cache::builder()
        .dir(PathBuf::from(
            env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into()),
        ))
        .build()
        .expect("Failed to create cache");

    let mut options = Options::default();
    options.extract = true;

    let extracted_dir = cache
        .cached_path_with_options(src, &options)
        .expect("Failed to fetch or cache UI archive");

    let extracted_build_path = PathBuf::from(&extracted_dir)
        .join("admin-dashboard-master")
        .join("build");

    println!(
        "cargo:rustc-env=CALIMERO_WEB_UI_PATH={}",
        extracted_build_path.display()
    );

    if Path::new(src).exists() {
        println!("cargo:rerun-if-changed={}", src);
    }
}
