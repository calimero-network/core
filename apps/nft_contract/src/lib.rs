use std::collections::HashMap;

use near_sdk::{env, near_bindgen, AccountId};

use near_sdk::borsh::{BorshDeserialize, BorshSerialize};

#[near_bindgen]
#[derive(BorshDeserialize, BorshSerialize)]
pub struct NFTContract {
    // Mapping from token ID to owner
    tokens: HashMap<String, AccountId>,
    // Tracking whether a user has minted an NFT
    user_minted: HashMap<AccountId, bool>,
}

impl Default for NFTContract {
    fn default() -> Self {
        Self {
            tokens: HashMap::new(),
            user_minted: HashMap::new(),
        }
    }
}

#[near_bindgen]
impl NFTContract {
    // Function to mint an NFT if the user hasn't minted one yet
    pub fn mint_if_not_minted(&mut self, token_id: String) {
        let account_id = env::signer_account_id();
        // Check if the user has already minted an NFT
        let has_minted = self.user_minted.get(&account_id).copied().unwrap_or(false);
        if !has_minted {
            // Mint the NFT
            self.tokens.insert(token_id.clone(), account_id.clone());
            self.user_minted.insert(account_id.clone(), true);
            env::log_str(&format!(
                "NFT with ID {} minted for account {}",
                token_id, account_id
            ));
        } else {
            env::log_str("User has already minted an NFT.");
        }
    }

    // Check if a user owns a specific NFT
    pub fn check_ownership(&self, token_id: String, user_id: AccountId) -> bool {
        match self.tokens.get(&token_id) {
            Some(owner) => *owner == user_id,
            None => false,
        }
    }

    // New method to check if the calling user has minted an NFT
    pub fn has_user_minted(&self) -> bool {
        let account_id = env::signer_account_id();
        self.user_minted.get(&account_id).copied().unwrap_or(false)
    }
}
