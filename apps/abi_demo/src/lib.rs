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

use abi_macros as abi;

/// Example error type
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("Invalid greeting: {0}")]
    InvalidGreeting(String),
}

/// Example SSApp module with ABI generation
#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    use super::*;
    
    /// Query function to get a greeting
    #[abi::query]
    pub fn get_greeting(name: String) -> String {
        format!("Hello, {}!", name)
    }
    
    /// Command function to set a greeting
    #[abi::command]
    pub fn set_greeting(new_value: String) -> Result<(), DemoError> {
        if new_value.is_empty() {
            return Err(DemoError::InvalidGreeting("Greeting cannot be empty".to_string()));
        }
        
        // In a real app, this would store the greeting
        println!("Setting greeting to: {}", new_value);
        Ok(())
    }
    
    /// Event emitted when greeting changes
    #[abi::event]
    pub struct GreetingChanged {
        pub old: String,
        pub new: String,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_get_greeting() {
        let result = demo::get_greeting("World".to_string());
        assert_eq!(result, "Hello, World!");
    }
    
    #[test]
    fn test_set_greeting_success() {
        let result = demo::set_greeting("Hello".to_string());
        assert!(result.is_ok());
    }
    
    #[test]
    fn test_set_greeting_error() {
        let result = demo::set_greeting("".to_string());
        assert!(result.is_err());
    }
} 