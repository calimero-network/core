//! Concurrent builds of one guest into one target directory must all succeed.

use std::path::Path;

use calimero_app_build::build_component;

#[test]
fn concurrent_builds_into_one_target_dir_succeed() {
    let guest = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/guests/echo");
    let target = tempfile::tempdir().unwrap();
    std::thread::scope(|s| {
        let builds: Vec<_> = (0..4)
            .map(|_| s.spawn(|| build_component(&guest, target.path())))
            .collect();
        for build in builds {
            let built = build.join().unwrap().unwrap();
            assert!(!built.component.is_empty());
        }
    });
}
