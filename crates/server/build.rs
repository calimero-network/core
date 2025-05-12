use std::path::{Path, PathBuf};
use std::{env, fs};

use cached_path::{Cache, Options};

const CALIMERO_WEB_UI_SRC: &str =
    "https://github.com/calimero-network/admin-dashboard/archive/refs/heads/master.zip";

fn main() {
    let src = option_env!("CALIMERO_WEB_UI_SRC").unwrap_or(CALIMERO_WEB_UI_SRC);

    let project_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let static_files_target = project_root.join("../../node-ui/build");

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

    if static_files_target.exists() {
        fs::remove_dir_all(&static_files_target).expect("Failed to clear old static files");
    }
    fs::create_dir_all(&static_files_target).expect("Failed to create target static files folder");

    fn copy_dir_all(src: &Path, dst: &Path) {
        for entry in fs::read_dir(src).unwrap() {
            let entry = entry.unwrap();
            let file_type = entry.file_type().unwrap();
            let dst_path = dst.join(entry.file_name());
            if file_type.is_dir() {
                fs::create_dir_all(&dst_path).unwrap();
                copy_dir_all(&entry.path(), &dst_path);
            } else {
                let _ = fs::copy(entry.path(), dst_path).expect("Failed to copy file");
            }
        }
    }

    copy_dir_all(&extracted_build_path, &static_files_target);

    if Path::new(src).exists() {
        println!("cargo:rerun-if-changed={}", src);
    }

    println!(
        "cargo:rustc-env=CALIMERO_WEB_UI_PATH={}",
        static_files_target.display()
    );
}
