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

/// Example error type for greeting operations
#[derive(Debug, thiserror::Error)]
pub enum DemoError {
    #[error("Invalid greeting: {0}")]
    InvalidGreeting(String),
    #[error("Greeting too long: {0}")]
    GreetingTooLong(usize),
}

/// Example error type for computation operations
#[derive(Debug, thiserror::Error)]
pub enum ComputeError {
    #[error("Division by zero")]
    DivisionByZero,
    #[error("Overflow occurred")]
    Overflow,
    #[error("Invalid input: {0}")]
    InvalidInput(String),
}

/// Example SSApp module with ABI generation
#[abi::module(name = "demo", version = "0.1.0")]
pub mod demo {
    use super::*;
    
    /// Query function to get a greeting (plain T return)
    #[abi::query]
    pub fn get_greeting(name: String) -> String {
        format!("Hello, {}!", name)
    }
    
    /// Command function to set a greeting (Result<(), E> return)
    #[abi::command]
    pub fn set_greeting(new_value: String) -> std::result::Result<(), DemoError> {
        if new_value.is_empty() {
            return Err(DemoError::InvalidGreeting("Greeting cannot be empty".to_string()));
        }
        
        if new_value.len() > 100 {
            return Err(DemoError::GreetingTooLong(new_value.len()));
        }
        
        // In a real app, this would store the greeting
        println!("Setting greeting to: {}", new_value);
        Ok(())
    }
    
    /// Query function to compute a value (Result<T, E> return)
    #[abi::query]
    pub fn compute(value: u64, divisor: u64) -> std::result::Result<u64, ComputeError> {
        if divisor == 0 {
            return Err(ComputeError::DivisionByZero);
        }
        
        if value > u64::MAX / 2 {
            return Err(ComputeError::Overflow);
        }
        
        if value == 0 {
            return Err(ComputeError::InvalidInput("Value cannot be zero".to_string()));
        }
        
        Ok(value / divisor)
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
    fn test_set_greeting_error_empty() {
        let result = demo::set_greeting("".to_string());
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DemoError::InvalidGreeting(_)));
    }
    
    #[test]
    fn test_set_greeting_error_too_long() {
        let long_greeting = "a".repeat(101);
        let result = demo::set_greeting(long_greeting);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), DemoError::GreetingTooLong(101)));
    }
    
    #[test]
    fn test_compute_success() {
        let result = demo::compute(10, 2);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), 5);
    }
    
    #[test]
    fn test_compute_division_by_zero() {
        let result = demo::compute(10, 0);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ComputeError::DivisionByZero));
    }
    
    #[test]
    fn test_compute_overflow() {
        let result = demo::compute(u64::MAX, 1);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ComputeError::Overflow));
    }
    
    #[test]
    fn test_compute_invalid_input() {
        let result = demo::compute(0, 1);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), ComputeError::InvalidInput(_)));
    }
} 