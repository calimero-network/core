use near_sdk::json_types::U128;
use near_sdk::store::LookupMap;
use near_sdk::{near, AccountId};

#[near(contract_state)]
pub struct LeaderBoard {
    scores: LookupMap<String, LookupMap<AccountId, U128>>, // Key is app name, value is the leader-board itself
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
    pub fn add_score(&mut self, app_name: String, account_id: AccountId, score: u128) {
        let app_leaderboard = self
            .scores
            .entry(app_name.to_string())
            .or_insert(LookupMap::new(app_name.as_bytes()));

        let new_score = app_leaderboard.entry(account_id.clone()).or_default().0 + score;
        app_leaderboard.insert(account_id, U128(new_score));
    }

    pub fn get_score(&self, app_name: String, account_id: AccountId) -> Option<u128> {
        if !self.scores.contains_key(&app_name) {
            return None;
        }

        self.scores
            .get(&app_name)
            .unwrap()
            .get(&account_id)
            .map(|score| score.0)
    }

    pub fn get_version(&self) -> String {
        "0.0.1".to_string()
    }
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use super::*;

    #[test]
    fn add_score() {
        let mut leader_board = LeaderBoard::default();
        let account = AccountId::from_str("alice.testnet").unwrap();
        leader_board.add_score("test_app".to_string(), account.clone(), 10);

        let score = leader_board.get_score("test_app".to_string(), account);
        assert_eq!(score, Some(10));
    }

    #[test]
    fn get_score_of_absent_account() {
        let mut leader_board = LeaderBoard::default();
        let account = AccountId::from_str("alice.testnet").unwrap();
        leader_board.add_score("test_app".to_string(), account.clone(), 10);

        let bob_account = AccountId::from_str("bob.testnet").unwrap();

        let score = leader_board.get_score("test_app".to_string(), bob_account);
        assert_eq!(score, None);
    }

    #[test]
    fn get_score_of_absent_app() {
        let mut leader_board = LeaderBoard::default();
        let account = AccountId::from_str("alice.testnet").unwrap();
        leader_board.add_score("test_app".to_string(), account.clone(), 10);

        let score = leader_board.get_score("test_app_2".to_string(), account);
        assert_eq!(score, None);
    }
}
