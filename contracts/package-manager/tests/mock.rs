#![cfg(not(target_arch = "wasm32"))]
#![allow(unused_crate_dependencies)]

use near_sdk::test_utils::VMContextBuilder;
use near_sdk::{testing_env, VMContext};
use package_manager::PackageManager;

fn get_context(is_view: bool) -> VMContext {
    VMContextBuilder::new()
        .signer_account_id("bobo".parse().unwrap())
        .is_view(is_view)
        .build()
}

#[test]
fn test_add_package() {
    let context = get_context(false);
    testing_env!(context);
    let mut contract = PackageManager::default();

    drop(contract.add_package(
        "application".to_string(),
        "Demo Application".to_string(),
        "https://github.com/application".to_string(),
    ));
    let package =
        contract.get_package("f50a6253c64e399051d942acc129c421cf1ccc591b7ba68f8e3365a23b201ce7");

    assert_eq!(package.owner, "bobo".to_string());
    assert_eq!(package.name, "application".to_string());
}

#[test]
fn test_add_release() {
    let context = get_context(false);
    testing_env!(context);
    let mut contract = PackageManager::default();

    drop(contract.add_package(
        "application".to_string(),
        "Demo Application".to_string(),
        "https://github.com/application".to_string(),
    ));
    contract.add_release(
        "application",
        "0.1.0".to_string(),
        "".to_string(),
        "https://gateway/ipfs/CID".to_string(),
        "123456789".to_string(),
    );
}

#[test]
fn test_get_packages_with_multiple_offsets_and_limits() {
    let mut contract = PackageManager::default();

    drop(contract.add_package(
        "application".to_string(),
        "Demo Application".to_string(),
        "https://github.com/application".to_string(),
    ));

    drop(contract.add_package(
        "package1".to_string(),
        "Package 1".to_string(),
        "https://github.com/package1".to_string(),
    ));

    drop(contract.add_package(
        "package2".to_string(),
        "Package 2".to_string(),
        "https://github.com/package2".to_string(),
    ));

    // Test with offset 0 and limit 1
    let packages_offset0_limit1 = contract.get_packages(0, 1);
    assert_eq!(packages_offset0_limit1.len(), 1);
    assert_eq!(packages_offset0_limit1[0].owner, "bob.near".to_string());
    assert_eq!(packages_offset0_limit1[0].name, "application".to_string());

    // Test with offset 1 and limit 1
    let packages_offset1_limit1 = contract.get_packages(1, 1);
    assert_eq!(packages_offset1_limit1.len(), 1);
    assert_eq!(packages_offset1_limit1[0].owner, "bob.near".to_string());
    assert_eq!(packages_offset1_limit1[0].name, "package1".to_string());

    // Test with offset 0 and limit 2
    let packages_offset0_limit2 = contract.get_packages(0, 2);
    assert_eq!(packages_offset0_limit2.len(), 2);
    assert_eq!(packages_offset0_limit2[0].name, "application".to_string());
    assert_eq!(packages_offset0_limit2[1].name, "package1".to_string());

    // Test with offset 1 and limit 2
    let packages_offset1_limit2 = contract.get_packages(1, 2);
    assert_eq!(packages_offset1_limit2.len(), 2);
    assert_eq!(packages_offset1_limit2[0].name, "package1".to_string());
    assert_eq!(packages_offset1_limit2[1].name, "package2".to_string());
}

#[test]
fn test_get_releases() {
    let mut contract = PackageManager::default();
    drop(contract.add_package(
        "application".to_string(),
        "Demo Application".to_string(),
        "https://github.com/application".to_string(),
    ));

    drop(contract.add_package(
        "package1".to_string(),
        "Package 1".to_string(),
        "https://github.com/package1".to_string(),
    ));
    contract.add_release(
        "application",
        "0.0.1".to_string(),
        "".to_string(),
        "https://gateway/ipfs/CID".to_string(),
        "123456789".to_string(),
    );
    contract.add_release(
        "application",
        "0.0.2".to_string(),
        "".to_string(),
        "https://gateway/ipfs/CID".to_string(),
        "123456789".to_string(),
    );
    contract.add_release(
        "application",
        "0.1.0".to_string(),
        "".to_string(),
        "https://gateway/ipfs/CID".to_string(),
        "123456789".to_string(),
    );
    contract.add_release(
        "package1",
        "0.1.1".to_string(),
        "".to_string(),
        "https://gateway/ipfs/CID".to_string(),
        "123456789".to_string(),
    );
    let app_releases_versions = contract.get_releases(
        "8ad69ecc5b424952a14859bb3b36c889bd0660cec342bc86aff35bfcaef9ba66",
        0,
        10,
    );
    let pkg_releases_versions = contract.get_releases(
        "3f5f73176789988dee4a989721aa147d63ca9bcde7b83bedf76e4772bf6448d5",
        0,
        10,
    );
    assert_eq!(app_releases_versions.len(), 3);
    assert_eq!(pkg_releases_versions.len(), 1);

    assert_eq!(app_releases_versions[2].version, "0.1.0".to_string());
    assert_eq!(pkg_releases_versions[0].version, "0.1.1".to_string());
}
