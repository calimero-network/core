use near_sdk::borsh::{BorshDeserialize, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::store::iterable_map::IterableMap;
use near_sdk::{env, near_bindgen, require, AccountId, BorshStorageKey};
use semver::Version;

#[derive(BorshSerialize, BorshStorageKey, Debug)]
#[borsh(crate = "near_sdk::borsh")]
#[non_exhaustive]
pub enum StorageKeys {
    Packages,
    Release { package: String },
    Releases,
}

// TODO: enable ABI generation support
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, Debug)]
#[borsh(crate = "near_sdk::borsh")]
#[non_exhaustive]
pub struct PackageManager {
    pub packages: IterableMap<String, Package>,
    pub releases: IterableMap<String, IterableMap<String, Release>>,
}

//  TODO: add multiple owners
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
#[borsh(crate = "near_sdk::borsh")]
#[non_exhaustive]
pub struct Package {
    pub id: String,
    pub name: String,
    pub description: String,
    pub repository: String,
    pub owner: AccountId,
}

// TODO: add a checksum in the future
// TODO: figure out status of reproduciable builds
// TODO: add better error checking for URL path
#[derive(BorshDeserialize, BorshSerialize, Clone, Debug, Deserialize, Serialize)]
#[serde(crate = "near_sdk::serde")]
#[borsh(crate = "near_sdk::borsh")]
#[non_exhaustive]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub path: String,
    pub hash: String,
}

impl Default for PackageManager {
    fn default() -> Self {
        Self {
            packages: IterableMap::new(StorageKeys::Packages),
            releases: IterableMap::new(StorageKeys::Releases),
        }
    }
}

#[near_bindgen]
impl PackageManager {
    pub fn add_package(&mut self, name: String, description: String, repository: String) -> String {
        let id_hash = Self::calculate_id_hash(&name);
        if self.packages.contains_key(&id_hash) {
            env::panic_str("Package already exists.")
        }

        drop(self.packages.insert(
            id_hash.clone(),
            Package::new(
                id_hash.clone(),
                name,
                description,
                repository,
                env::signer_account_id(),
            ),
        ));
        id_hash
    }

    fn calculate_id_hash(name: &str) -> String {
        hex::encode(env::sha256(
            format!("{}:{}", name, env::signer_account_id()).as_bytes(),
        ))
    }

    pub fn add_release(
        &mut self,
        name: String,
        version: String,
        notes: String,
        path: String,
        hash: String,
    ) {
        let id_hash = Self::calculate_id_hash(&name);
        // Get the last release version for the package
        let last_release_version = self.releases.get(&id_hash).map(|version_map| {
            version_map
                .keys()
                .max_by(|a, b| Version::parse(a).unwrap().cmp(&Version::parse(b).unwrap()))
                .expect("No versions found for the package")
        });

        // Check if the last release version exists and is less than the current version
        if let Some(last_version) = last_release_version {
            let last_version =
                Version::parse(last_version).expect("Failed to parse last release version");
            let current_version =
                Version::parse(&version).expect("Failed to parse current version");
            if current_version <= last_version {
                env::panic_str(
                    "New release version must be greater than the last release version.",
                );
            }
        }

        // Check if the sender is the owner of the package
        let package = self.packages.get(&id_hash).expect("Package doesn't exist.");
        if package.owner != env::signer_account_id() {
            env::panic_str("Sender is not the owner of the package");
        }

        // Insert the new release
        drop(
            self.releases
                .entry(id_hash.clone())
                .or_insert_with(|| {
                    IterableMap::new(StorageKeys::Release {
                        package: id_hash.clone(),
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
                ),
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

    pub fn get_releases(&self, id: String, offset: usize, limit: usize) -> Vec<&Release> {
        self.releases
            .get(&id)
            .expect("Package doesn't exist.")
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(_, release)| release)
            .collect()
    }

    pub fn get_package(&self, id: String) -> &Package {
        self.packages.get(&id).expect("Package doesn't exist")
    }

    pub fn get_release(&self, id: String, version: String) -> &Release {
        self.releases
            .get(&id)
            .expect("Package doesn't exist")
            .get(&version)
            .expect("Version doesn't exist")
    }

    pub fn erase(&mut self) {
        require!(
            env::signer_account_id() == env::current_account_id(),
            "Not so fast, chief.."
        );

        self.packages.clear();
        for (_, mut releases) in self.releases.drain() {
            releases.clear();
        }
    }
}

impl Package {
    const fn new(
        id: String,
        name: String,
        description: String,
        repository: String,
        owner: AccountId,
    ) -> Self {
        Self {
            id,
            name,
            description,
            repository,
            owner,
        }
    }
}

#[cfg(test)]
mod integration_tests_package_usage {
    use {near_workspaces as _, serde_json as _, tokio as _};
}
