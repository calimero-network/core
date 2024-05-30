use std::collections::BTreeMap;

use near_sdk::json_types::U128;
use near_sdk::near;
use near_sdk::store::{LookupMap, UnorderedMap};

type UserName = String;

#[near(contract_state)]
pub struct LeaderBoard {
    scores: LookupMap<String, UnorderedMap<UserName, U128>>, // Key is app name, value is the leaderboard itself
}

impl Default for LeaderBoard {
    fn default() -> Self {
        Self {
            scores: LookupMap::new(b"m"),
        }
    }
}

#[near]
impl LeaderBoard {
    pub fn add_score(&mut self, app_name: String, account_id: UserName, score: U128) {
        let app_leaderboard = self
            .scores
            .entry(app_name.clone())
            .or_insert_with(|| UnorderedMap::new(app_name.as_bytes()));

        let new_score = app_leaderboard.entry(account_id.clone()).or_default().0 + score.0;
        app_leaderboard.insert(account_id, U128(new_score));
    }

    pub fn get_score(&self, app_name: String, account_id: UserName) -> Option<U128> {
        self.scores
            .get(&app_name)?
            .get(&account_id)
            .map(|score| score.clone())
    }

    pub fn get_scores(&self, app_name: String) -> Option<BTreeMap<String, u128>> {
        let mut map = BTreeMap::new();
        for (k, v) in self.scores.get(&app_name)? {
            map.insert(k.to_string(), v.0);
        }
        Some(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_score() {
        let mut leader_board = LeaderBoard::default();
        let account = "alice.testnet".to_string();
        leader_board.add_score("test_app".to_string(), account.clone(), U128(10));

        let score = leader_board.get_score("test_app".to_string(), account);
        assert_eq!(score, Some(U128(10)));
    }

    #[test]
    fn get_score_of_absent_account() {
        let mut leader_board = LeaderBoard::default();
        let account = "alice.testnet".to_string();
        leader_board.add_score("test_app".to_string(), account.clone(), U128(10));

        let bob_account = "bob.testnet".to_string();

        let score = leader_board.get_score("test_app".to_string(), bob_account);
        assert_eq!(score, None);
    }

    #[test]
    fn get_score_of_absent_app() {
        let mut leader_board = LeaderBoard::default();
        let account = "alice.testnet".to_string();
        leader_board.add_score("test_app".to_string(), account.clone(), U128(10));

        let score = leader_board.get_score("test_app_2".to_string(), account);
        assert_eq!(score, None);
    }
}
