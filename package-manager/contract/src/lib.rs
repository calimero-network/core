use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::store::UnorderedMap;
use near_sdk::{env, near_bindgen, AccountId, BorshStorageKey};
use semver;

#[derive(BorshStorageKey, BorshSerialize)]
#[borsh(crate = "near_sdk::borsh")]
pub enum StorageKeys {
    Packages,
    Release { package: String },
    Releases,
}

// TODO: enable ABI generation support
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
#[borsh(crate = "near_sdk::borsh")]
pub struct PackageManager {
    packages: UnorderedMap<String, Package>,
    releases: UnorderedMap<String, UnorderedMap<String, Release>>,
}

//  TODO: add multiple owners
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
#[borsh(crate = "near_sdk::borsh")]
pub struct Package {
    name: String,
    description: String,
    repository: String,
    owner: AccountId,
}

// TODO: add a checksum in the future
// TODO: figure out status of reproduciable builds
// TODO: add better error checking for URL path
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
#[borsh(crate = "near_sdk::borsh")]
pub struct Release {
    version: String,
    notes: String,
    path: String,
    hash: String,
}

impl Default for PackageManager {
    fn default() -> Self {
        Self {
            packages: UnorderedMap::new(StorageKeys::Packages),
            releases: UnorderedMap::new(StorageKeys::Releases),
        }
    }
}

#[near_bindgen]
impl PackageManager {
    pub fn add_package(&mut self, name: String, description: String, repository: String) {
        if self.packages.contains_key(&name) {
            panic!("Package already exists.")
        }

        self.packages.insert(
            name.clone(),
            Package::new(name, description, repository, env::signer_account_id()),
        );
    }

    pub fn add_release(
        &mut self,
        name: String,
        version: String,
        notes: String,
        path: String,
        hash: String,
    ) {
        // Get the last release version for the package
        let last_release_version = self.releases.get(&name).map(|version_map| {
            version_map
                .keys()
                .max_by(|a, b| {
                    semver::Version::parse(a)
                        .unwrap()
                        .cmp(&semver::Version::parse(b).unwrap())
                })
                .expect("No versions found for the package")
        });

        // Check if the last release version exists and is less than the current version
        if let Some(last_version) = last_release_version {
            let last_version = semver::Version::parse(&last_version)
                .expect("Failed to parse last release version");
            let current_version =
                semver::Version::parse(&version).expect("Failed to parse current version");
            if current_version <= last_version {
                panic!("New release version must be greater than the last release version.");
            }
        }

        // Check if the sender is the owner of the package
        let package = self.packages.get(&name).expect("Package doesn't exist.");
        if package.owner != env::signer_account_id() {
            panic!("Sender is not the owner of the package");
        }

        // Insert the new release
        self.releases
            .entry(name.clone())
            .or_insert_with(|| {
                UnorderedMap::new(StorageKeys::Release {
                    package: name.clone(),
                })
            })
            .insert(
                version.clone(),
                Release {
                    version,
                    notes,
                    path,
                    hash,
                },
            );
    }

    pub fn get_packages(&self, offset: usize, limit: usize) -> Vec<Package> {
        self.packages
            .keys()
            .skip(offset)
            .take(limit)
            .filter_map(|key| self.packages.get(key).cloned())
            .collect()
    }

    pub fn get_releases(&self, offset: usize, limit: usize) -> Vec<Release> {
        self.releases
            .values()
            .flat_map(|version_map| version_map.values().cloned())
            .skip(offset)
            .take(limit)
            .collect()
    }

    pub fn get_package(&self, name: String) -> &Package {
        self.packages.get(&name).expect("Package doesn't exist")
    }

    pub fn get_release(&self, name: String, version: String) -> &Release {
        self.releases
            .get(&name)
            .expect("Package doesn't exist")
            .get(&version)
            .expect("Version doesn't exist")
    }
}

impl Package {
    fn new(name: String, description: String, repository: String, owner: AccountId) -> Self {
        Self {
            name: name,
            description: description,
            repository: repository,
            owner: owner,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[cfg(test)]
mod tests {
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, VMContext};

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

        contract.add_package(
            "application".to_string(),
            "Demo Application".to_string(),
            "https://github.com/application".to_string(),
        );
        let package = contract.get_package("application".to_string());

        assert_eq!(package.owner, "bobo".to_string());
        assert_eq!(package.name, "application".to_string());
    }

    #[test]
    fn test_add_release() {
        let context = get_context(false);
        testing_env!(context);
        let mut contract = PackageManager::default();

        contract.add_package(
            "application".to_string(),
            "Demo Application".to_string(),
            "https://github.com/application".to_string(),
        );
        contract.add_release(
            "application".to_string(),
            "0.1.0".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID".to_string(),
            "123456789".to_string(),
        );
    }

    #[test]
    fn test_get_packages_with_multiple_offsets_and_limits() {
        let mut contract = PackageManager::default();

        contract.add_package(
            "application".to_string(),
            "Demo Application".to_string(),
            "https://github.com/application".to_string(),
        );

        contract.add_package(
            "package1".to_string(),
            "Package 1".to_string(),
            "https://github.com/package1".to_string(),
        );

        contract.add_package(
            "package2".to_string(),
            "Package 2".to_string(),
            "https://github.com/package2".to_string(),
        );

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
    fn test_get_realses() {
        let mut contract = PackageManager::default();
        contract.add_package(
            "application".to_string(),
            "Demo Application".to_string(),
            "https://github.com/application".to_string(),
        );

        contract.add_package(
            "package1".to_string(),
            "Package 1".to_string(),
            "https://github.com/package1".to_string(),
        );
        contract.add_release(
            "application".to_string(),
            "0.0.1".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID".to_string(),
            "123456789".to_string(),
        );
        contract.add_release(
            "application".to_string(),
            "0.0.2".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID".to_string(),
            "123456789".to_string(),
        );
        contract.add_release(
            "application".to_string(),
            "0.1.0".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID".to_string(),
            "123456789".to_string(),
        );
        contract.add_release(
            "package1".to_string(),
            "0.1.1".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID".to_string(),
            "123456789".to_string(),
        );
        let relases_versions = contract.get_releases(1, 2);
        assert_eq!(relases_versions.len(), 2);
        assert_eq!(relases_versions[0].version, "0.0.2".to_string());
    }
}
