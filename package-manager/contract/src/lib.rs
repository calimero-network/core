use near_sdk::borsh::{self, BorshDeserialize, BorshSerialize};
use near_sdk::serde::{Deserialize, Serialize};
use near_sdk::store::LookupMap;
use near_sdk::{env, near_bindgen, AccountId, BorshStorageKey};

#[derive(BorshStorageKey, BorshSerialize)]
pub enum StorageKeys {
    Packages,
    Release,
    Releases,
}

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct PackageManager {
    packages: LookupMap<String, Package>,
    releases: LookupMap<String, LookupMap<String, Release>>,
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
            releases: LookupMap::new(StorageKeys::Releases),
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
        let Some(package) = self.packages.get(&name) else {
            env::panic("Package doesn't exist.");
        };
        if package.owner != env::signer_account_id() {
            panic!("Sender is not the owner of the package");
        }

        if !self.releases.contains_key(&name) {
            self.releases
                .insert(name.clone(), LookupMap::new(StorageKeys::Release));
        }

        self.releases.get_mut(&name).unwrap().insert(
            version.clone(),
            Release {
                version,
                notes,
                path,
                hash,
            },
        );
    }

    // TODO: implement `pub fn get_packages(&self) -> Vec<Package> {}`

    // TODO: implement `pub fn get_releases(&self) -> Vec<Release> {}`

    pub fn get_package(&self, name: String) -> &Package {
        self.packages
            .get(&name)
            .expect("Package doesn't exist")
    }

    pub fn get_release(&self, name: String, version: String) -> Release {
        self.releases
            .get(&name)
            .expect("Package doesn't exist")
            .get(&version)
            .expect("Version doesn't exist")
            .clone()
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
    use super::*;
    use near_sdk::test_utils::VMContextBuilder;
    use near_sdk::MockedBlockchain;
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
            "https://shithub.com/application".to_string(),
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
            "https://shithub.com/application".to_string(),
        );
        contract.add_release(
            "application".to_string(),
            "0.1.0".to_string(),
            "".to_string(),
            "https://gateway/ipfs/CID".to_string(),
            "123456789".to_string(),
        );
    }
}
