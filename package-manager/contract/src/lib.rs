use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::store::LookupMap;
use near_sdk::{env, near_bindgen, AccountId, BorshStorageKey};

#[derive(BorshStorageKey, BorshSerialize)]
pub enum StorageKeys {
    Packages,
    Release { package: String },
    Releases,
    Versions,
}

// TODO: enable ABI generation support
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct PackageManager {
    packages: LookupMap<String, Package>,
    package_keys: Vec<String>,
    releases: LookupMap<String, LookupMap<String, Release>>,
    release_keys: Vec<String>,
    release_versions: LookupMap<String, Vec<String>>,
}

//  TODO: add multiple owners
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Package {
    name: String,
    description: String,
    repository: String,
    owner: AccountId,
}

// TODO: add a checksum in the future
// TODO: figure out status of reproduciable builds
// TODO: add better error checking for URL path
#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize, Serialize, Deserialize, Clone)]
#[serde(crate = "near_sdk::serde")]
pub struct Release {
    version: String,
    notes: String,
    path: String,
    hash: String,
}

impl Default for PackageManager {
    fn default() -> Self {
        Self {
            packages: LookupMap::new(StorageKeys::Packages),
            package_keys: Vec::new(),
            releases: LookupMap::new(StorageKeys::Releases),
            release_keys: Vec::new(),
            release_versions: LookupMap::new(StorageKeys::Versions),
        }
    }
}

#[near_bindgen]
impl PackageManager {
    pub fn add_package(&mut self, name: String, description: String, repository: String) {
        if self.packages.contains_key(&name) {
            panic!("Package already exists.")
        }
        self.package_keys.push(name.clone());
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
        let Some(package) = self.packages.get(&name) else {
            env::panic_str("Package doesn't exist.");
        };
        if package.owner != env::signer_account_id() {
            env::panic_str("Sender is not the owner of the package");
        }

        if !self.release_keys.contains(&name) {
            self.release_keys.push(name.clone());
        }

        let versions_vec = self.release_versions.entry(name.clone()).or_insert(Vec::new());

        versions_vec.push(version.clone());

        self.releases
            .entry(name.clone())
            .or_insert_with(|| {
                LookupMap::new(StorageKeys::Release {
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

    pub fn get_packages(&self, offset: u64, limit: u64) -> Vec<Package> {
        let offset_usize = offset as usize;
        let limit_usize = limit as usize;
        let end_index = (offset_usize + limit_usize).min(self.package_keys.len());
        let keys = self.package_keys[offset_usize..end_index].to_vec();
        let mut result = Vec::with_capacity(keys.len());

        for key in keys {
            if let Some(package) = self.packages.get(&key) {
                result.push(package.clone());
            }
        }
        //ALSO RETURN OFFSET IF NEEDED

        result
    }

    pub fn get_releases(&self, offset: u64, limit: u64) -> Vec<Release> {
        let offset_usize = offset as usize;
        let limit_usize = limit as usize;
        let end_index = (offset_usize + limit_usize).min(self.release_keys.len());
        let release_keys = self.release_keys[offset_usize..end_index].to_vec();
        let mut result = Vec::with_capacity(release_keys.len());

        for name in release_keys {
            if let Some(versions) = self.release_versions.get(&name) {
                for version in versions {
                    let release = self.get_release(name.clone(), version.to_string());
                    result.push(release.clone());
                }
                
            }
        }

        result
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

#[cfg(test)]
mod tests {
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::{testing_env, MockedBlockchain, VMContext};

    use super::*;

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

        assert_eq!(package.owner, "bobo".parse().unwrap());
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
    fn test_get_packages_pagination() {
        let context = get_context(false);
        testing_env!(context);
        let mut contract = PackageManager::default();

        contract.add_package(
            "package1".to_string(),
            "Description 1".to_string(),
            "https://github.com/package1".to_string(),
        );
        contract.add_package(
            "package2".to_string(),
            "Description 2".to_string(),
            "https://github.com/package2".to_string(),
        );
        contract.add_package(
            "package3".to_string(),
            "Description 3".to_string(),
            "https://github.com/package3".to_string(),
        );

        let packages_page1 = contract.get_packages(0, 2);
        assert_eq!(packages_page1.len(), 2);
        assert_eq!(packages_page1[0].name, "package1");
        assert_eq!(packages_page1[1].name, "package2");

        let packages_page2 = contract.get_packages(1, 2);
        assert_eq!(packages_page2.len(), 2);
        assert_eq!(packages_page2[0].name, "package2");
        assert_eq!(packages_page2[1].name, "package3");

        let packages_page3 = contract.get_packages(2, 2);
        assert_eq!(packages_page3.len(), 1);
        assert_eq!(packages_page3[0].name, "package3");
    }

    #[test]
    fn test_get_releases() {
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
            "https://gateway/ipfs/CID1".to_string(),
            "123456789".to_string(),
        );
        contract.add_release(
            "application".to_string(),
            "0.2.0".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID2".to_string(),
            "987654321".to_string(),
        );

        let releases = contract.get_releases(0, 2);
        assert_eq!(releases.len(), 2);

        assert_eq!(releases[0].version, "0.1.0");
        assert_eq!(releases[0].path, "https://gateway/ipfs/CID1");
        assert_eq!(releases[0].hash, "123456789");

        assert_eq!(releases[1].version, "0.2.0");
        assert_eq!(releases[1].path, "https://gateway/ipfs/CID2");
        assert_eq!(releases[1].hash, "987654321");

        contract.add_package(
            "application2".to_string(),
            "Demo Application".to_string(),
            "https://github.com/application".to_string(),
        );

        contract.add_release(
            "application2".to_string(),
            "0.2.0".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID2".to_string(),
            "987654321".to_string(),
        );

        let releases2 = contract.get_releases(1, 10);
        assert_eq!(releases.len(), 2);
        assert_eq!(releases[0].hash, "123456789");
    }
}
