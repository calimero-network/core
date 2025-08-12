// Copyright 2024 Calimero Limited
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::collections::BTreeMap;
use calimero_sdk::app;

/// Advanced type for showcasing v0.1.1 dual-mode maps
pub type ScoreMap = BTreeMap<String, u64>;

/// Example error type for greeting operations
#[derive(Debug, thiserror::Error, calimero_sdk::serde::Serialize)]
#[serde(crate = "calimero_sdk::serde")]
#[serde(tag = "kind", content = "data")]
pub enum DemoError {
    #[error("Greeting cannot be empty")]
    Empty,
    #[error("Greeting too long: max {max}, got {got}")]
    TooLong { max: u8, got: u8 },
}

/// Example SSApp module with ABI generation
#[app::state(emits = DemoEvent)]
#[derive(Debug, borsh::BorshSerialize, borsh::BorshDeserialize)]
pub struct DemoApp {
    greeting: String,
    scores: ScoreMap,
}

#[derive(Debug)]
#[app::event]
pub enum DemoEvent {
    GreetingChanged { old: String, new: String },
}

#[app::logic]
impl DemoApp {
    #[app::init]
    pub fn init() -> Self {
        Self {
            greeting: "Hello, World!".to_string(),
            scores: BTreeMap::new(),
        }
    }
    
    /// Query function to get a greeting (plain T return)
    pub fn get_greeting(&self, name: String) -> String {
        format!("Hello, {}!", name)
    }
    
    /// Command function to set a greeting (Result<(), E> return)
    pub fn set_greeting(&mut self, new_value: String) -> app::Result<(), DemoError> {
        if new_value.is_empty() {
            return Err(DemoError::Empty);
        }
        
        if new_value.len() > 100 {
            return Err(DemoError::TooLong { 
                max: 100, 
                got: new_value.len() as u8 
            });
        }
        
        let old_greeting = self.greeting.clone();
        self.greeting = new_value.clone();
        
        app::emit!(DemoEvent::GreetingChanged {
            old: old_greeting,
            new: new_value,
        });
        
        Ok(())
    }
    
    /// Query function using advanced type (ScoreMap)
    pub fn get_scores(&self) -> ScoreMap {
        self.scores.clone()
    }
    
    /// Command function to set a score
    pub fn set_score(&mut self, player: String, score: u64) -> app::Result<()> {
        self.scores.insert(player, score);
        Ok(())
    }
} 