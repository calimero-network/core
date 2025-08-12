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
use abi_core::AbiType;

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

#[cfg(feature = "abi-conformance")]
pub mod conformance {
    use abi_macros as abi;
    use abi_core::AbiType;
    use std::collections::BTreeMap;
    
    /// Example struct with various field types
    #[derive(abi::AbiType)]
    pub struct ComplexStruct {
        pub id: u64,
        pub name: String,
        pub data: Option<Vec<u8>>,
        pub metadata: BTreeMap<String, u64>,
    }
    
    /// Example newtype struct
    #[derive(abi::AbiType)]
    pub struct UserId(u128);
    
    /// Example enum with different variant types
    #[derive(abi::AbiType)]
    pub enum Status {
        Pending,
        Active(u32),
        Completed { timestamp: u64, result: String },
    }
    
    /// Example error type for advanced operations
    #[derive(Debug, thiserror::Error)]
    pub enum AdvancedError {
        #[error("Invalid status: {0}")]
        InvalidStatus(String),
        #[error("Resource not found: {0}")]
        NotFound(u64),
        #[error("Operation failed")]
        OperationFailed,
    }
    
    /// Conformance module with advanced types
    #[abi::module(name = "conformance", version = "0.1.0")]
    pub mod advanced {
        use super::*;
        
        /// Query function using complex struct
        #[abi::query]
        pub fn get_user_info(user_id: UserId) -> std::result::Result<ComplexStruct, AdvancedError> {
            if user_id.0 == 0 {
                return Err(AdvancedError::NotFound(user_id.0));
            }
            
            Ok(ComplexStruct {
                id: user_id.0,
                name: "Test User".to_string(),
                data: Some(vec![1, 2, 3, 4]),
                metadata: {
                    let mut map = BTreeMap::new();
                    map.insert("created".to_string(), 1234567890);
                    map.insert("updated".to_string(), 1234567890);
                    map
                },
            })
        }
        
        /// Command function using enum and tuple
        #[abi::command]
        pub fn update_status(
            user_id: UserId,
            status: Status,
            coords: (u8, String),
        ) -> std::result::Result<[u16; 4], AdvancedError> {
            if user_id.0 == 0 {
                return Err(AdvancedError::NotFound(user_id.0));
            }
            
            match status {
                Status::Pending => {
                    if coords.0 > 100 {
                        return Err(AdvancedError::InvalidStatus("Invalid coordinate".to_string()));
                    }
                }
                Status::Active(_) => {
                    // Valid status
                }
                Status::Completed { .. } => {
                    return Err(AdvancedError::OperationFailed);
                }
            }
            
            Ok([1, 2, 3, 4])
        }
        
        /// Event with complex payload
        #[abi::event]
        pub struct UserStatusChanged {
            pub user_id: UserId,
            pub old_status: Status,
            pub new_status: Status,
        }
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
    
    #[cfg(feature = "abi-conformance")]
    mod conformance_tests {
        use super::*;
        use conformance::{UserId, ComplexStruct, Status, AdvancedError};
        
        #[test]
        fn test_get_user_info_success() {
            let user_id = UserId(123);
            let result = conformance::advanced::get_user_info(user_id);
            assert!(result.is_ok());
            
            let user = result.unwrap();
            assert_eq!(user.id, 123);
            assert_eq!(user.name, "Test User");
            assert_eq!(user.data, Some(vec![1, 2, 3, 4]));
            assert_eq!(user.metadata.len(), 2);
        }
        
        #[test]
        fn test_get_user_info_not_found() {
            let user_id = UserId(0);
            let result = conformance::advanced::get_user_info(user_id);
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), AdvancedError::NotFound(0)));
        }
        
        #[test]
        fn test_update_status_success() {
            let user_id = UserId(123);
            let status = Status::Active(42);
            let coords = (50, "test".to_string());
            
            let result = conformance::advanced::update_status(user_id, status, coords);
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), [1, 2, 3, 4]);
        }
        
        #[test]
        fn test_update_status_invalid_coordinate() {
            let user_id = UserId(123);
            let status = Status::Pending;
            let coords = (150, "test".to_string());
            
            let result = conformance::advanced::update_status(user_id, status, coords);
            assert!(result.is_err());
            assert!(matches!(result.unwrap_err(), AdvancedError::InvalidStatus(_)));
        }
    }
} 