use std::path::{Path, PathBuf};
use std::{env, fs};

use cached_path::{Cache, Options};
use zip::ZipArchive;

fn main() {
    let src = option_env!("CALIMERO_WEB_UI_SRC").unwrap_or(
        "https://github.com/calimero-network/admin-dashboard/archive/refs/heads/master.zip",
    );

    let force = option_env!("CALIMERO_WEB_UI_FETCH")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);

    let project_root = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let static_files_target = project_root.join("../../node-ui/build");
    let marker_file = project_root.join("build-deps-changed.txt");

    let target_missing = !Path::new("target").exists();
    let node_ui_missing = !static_files_target.exists();

    if target_missing || node_ui_missing {
        eprintln!("Triggering rebuild because `target/` or `node-ui/build` is missing.");
        fs::write(
            &marker_file,
            format!("trigger: {:?}\n", std::time::SystemTime::now()),
        )
        .expect("Failed to write marker file");
    }

    let cache = Cache::builder()
        .dir(PathBuf::from(
            env::var("CARGO_TARGET_DIR").unwrap_or_else(|_| "target".into()),
        ))
        .build()
        .expect("Failed to create cache");

    let options = if force {
        Options::default().extract()
    } else {
        Options::default()
    };

    let archive_path = cache
        .cached_path_with_options(src, &options)
        .expect("Failed to fetch or cache UI archive");

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap()).join("web-ui");
    if out_dir.exists() {
        fs::remove_dir_all(&out_dir).expect("Failed to remove existing output directory");
    }
    fs::create_dir_all(&out_dir).expect("Failed to create output directory");

    let zip_file = fs::File::open(&archive_path).expect("Cannot open downloaded ZIP archive");
    let mut zip = ZipArchive::new(zip_file).expect("Failed to read ZIP archive");
    zip.extract(&out_dir)
        .expect("Failed to extract ZIP archive");

    let extracted_build_path = out_dir.join("admin-dashboard-master").join("build");

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

    println!("cargo:rerun-if-env-changed=CALIMERO_WEB_UI_SRC");
    println!("cargo:rerun-if-env-changed=CALIMERO_WEB_UI_FETCH");

    if Path::new(src).exists() {
        println!("cargo:rerun-if-changed={}", src);
    }

    println!("cargo:rerun-if-changed={}", marker_file.display());

    println!("cargo:rerun-if-changed=build.rs");

    println!(
        "cargo:rustc-env=CALIMERO_WEB_UI_PATH={}",
        static_files_target.display()
    );
}
