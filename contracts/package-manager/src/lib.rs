use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::store::UnorderedMap;
use near_sdk::{env, near_bindgen, AccountId, BorshStorageKey};

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
    pub packages: UnorderedMap<String, Package>,
    pub releases: UnorderedMap<String, UnorderedMap<String, Release>>,
}

//  TODO: add multiple owners
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
#[borsh(crate = "near_sdk::borsh")]
pub struct Package {
    pub name: String,
    pub description: String,
    pub repository: String,
    pub owner: AccountId,
}

// TODO: add a checksum in the future
// TODO: figure out status of reproduciable builds
// TODO: add better error checking for URL path
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
#[borsh(crate = "near_sdk::borsh")]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub path: String,
    pub hash: String,
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
            env::panic_str("Package already exists.")
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
                env::panic_str(
                    "New release version must be greater than the last release version.",
                );
            }
        }

        // Check if the sender is the owner of the package
        let package = self.packages.get(&name).expect("Package doesn't exist.");
        if package.owner != env::signer_account_id() {
            env::panic_str("Sender is not the owner of the package");
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

    pub fn get_releases(&self, name: String, offset: usize, limit: usize) -> Vec<&Release> {
        self.releases
            .get(&name)
            .expect("Package doesn't exist.")
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(_, release)| release)
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
