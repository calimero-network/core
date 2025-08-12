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
use abi_demo::{DemoApp, DemoError, ScoreMap};

#[test]
fn test_runtime_smoke() {
    // Initialize the app
    let mut app = DemoApp::init();
    
    // Test 1: Initialize state with set_greeting
    let result = app.set_greeting("hello".to_string(), None);
    assert!(result.is_ok(), "Failed to set initial greeting");
    
    // Test 2: Query get_greeting returns expected format
    let greeting = app.get_greeting("world".to_string());
    assert_eq!(greeting, "hello, world!", "Unexpected greeting format");
    
    // Test 3: Trigger error path (empty greeting)
    let error_result = app.set_greeting("".to_string(), None);
    assert!(error_result.is_err(), "Empty greeting should return error");
    
    let error = error_result.unwrap_err();
    assert!(matches!(error, DemoError::Empty), "Expected Empty error, got {:?}", error);
    
    // Test 4: Test with scores parameter (advanced type)
    let mut scores = ScoreMap::new();
    scores.insert("alice".to_string(), 100);
    scores.insert("bob".to_string(), 85);
    
    let result = app.set_greeting("greetings".to_string(), Some(scores));
    assert!(result.is_ok(), "Failed to set greeting with scores");
    
    // Test 5: Verify greeting was updated
    let greeting = app.get_greeting("everyone".to_string());
    assert_eq!(greeting, "greetings, everyone!", "Greeting should be updated");
    
    // Test 6: Trigger too long error
    let long_greeting = "a".repeat(101);
    let error_result = app.set_greeting(long_greeting, None);
    assert!(error_result.is_err(), "Too long greeting should return error");
    
    let error = error_result.unwrap_err();
    assert!(matches!(error, DemoError::TooLong { max: 100, got: 101 }), 
            "Expected TooLong error, got {:?}", error);
}

#[test]
fn test_event_emission() {
    // This test would require a more sophisticated runtime harness
    // to capture events. For now, we test that the app compiles and
    // functions work as expected.
    
    let mut app = DemoApp::init();
    
    // Set initial greeting
    let result = app.set_greeting("initial".to_string(), None);
    assert!(result.is_ok());
    
    // Change greeting (should emit GreetingChanged event)
    let result = app.set_greeting("updated".to_string(), None);
    assert!(result.is_ok());
    
    // Verify the greeting was actually changed
    let greeting = app.get_greeting("test".to_string());
    assert_eq!(greeting, "updated, test!", "Greeting should be updated");
}

#[test]
fn test_score_map_advanced_type() {
    let mut app = DemoApp::init();
    
    // Test with empty scores
    let result = app.set_greeting("test".to_string(), Some(ScoreMap::new()));
    assert!(result.is_ok());
    
    // Test with populated scores
    let mut scores = ScoreMap::new();
    scores.insert("player1".to_string(), 1000);
    scores.insert("player2".to_string(), 850);
    scores.insert("player3".to_string(), 1200);
    
    let result = app.set_greeting("game".to_string(), Some(scores));
    assert!(result.is_ok());
    
    // Verify greeting was set
    let greeting = app.get_greeting("players".to_string());
    assert_eq!(greeting, "game, players!", "Greeting should be set with scores");
} 