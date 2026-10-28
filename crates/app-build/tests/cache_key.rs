use std::fs;
use std::path::Path;

use calimero_app_build::cache_key;

fn crate_copy(root: &Path) -> std::path::PathBuf {
    let dir = root.join("guest");
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("Cargo.toml"), "[package]\nname = \"g\"\n").unwrap();
    fs::write(dir.join("src/lib.rs"), "// a\n").unwrap();
    dir
}

#[test]
fn a_hashed_cargo_toml_changes_the_key_and_target_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let guest = crate_copy(&tmp.path().join("one"));
    let dep = crate_copy(&tmp.path().join("two"));
    let base = cache_key(&guest, &[&dep]).unwrap();
    assert_eq!(base, cache_key(&guest, &[&dep]).unwrap());

    // The same tree elsewhere has the same key (paths are relative to their roots).
    let guest2 = crate_copy(&tmp.path().join("three"));
    assert_eq!(base, cache_key(&guest2, &[&dep]).unwrap());

    // An unrelated file under target/ does not move it.
    fs::create_dir_all(dep.join("target/debug")).unwrap();
    fs::write(dep.join("target/debug/junk"), "x").unwrap();
    assert_eq!(base, cache_key(&guest, &[&dep]).unwrap());

    // One byte in a dependency's Cargo.toml does.
    fs::write(dep.join("Cargo.toml"), "[package]\nname = \"h\"\n").unwrap();
    assert_ne!(base, cache_key(&guest, &[&dep]).unwrap());

    // So does one byte in the guest's own sources.
    let dep_key = cache_key(&guest, &[&dep]).unwrap();
    fs::write(guest.join("src/lib.rs"), "// b\n").unwrap();
    assert_ne!(dep_key, cache_key(&guest, &[&dep]).unwrap());
}
