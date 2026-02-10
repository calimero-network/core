//! Private storage utilities for Calimero applications.
//!
//! This module provides utilities for managing private application state
//! using the Calimero storage system. It includes utilities for reading/writing
//! state and managing state references.
//!
//! # Examples
//!
//! ## Using the `#[app::private]` macro (Recommended)
//!
//! The `#[app::private]` macro automatically generates storage keys and helper methods.
//! Simply add `#[app::private]` to your struct:
//!
//! ```text
//! #[derive(Default, BorshSerialize, BorshDeserialize, Debug)]
//! #[borsh(crate = "calimero_sdk::borsh")]
//! #[app::private] // Automatically generates storage key and helper methods
//! pub struct MyState {
//!     counter: u32,
//!     data: String,
//! }
//! ```
//!
//! This generates methods like `MyState::private_load_or_default()` for easy usage.
//!
//! ## Manual usage (Advanced)
//!
//! ```rust
//! use calimero_sdk::{borsh, private_storage};
//! use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
//!
//! #[derive(Default, BorshSerialize, BorshDeserialize, Debug)]
//! #[borsh(crate = "calimero_sdk::borsh")]
//! pub struct MyState {
//!     counter: u32,
//!     data: String,
//! }
//!
//! // Usage in your application
//! fn example_usage() -> calimero_sdk::app::Result<()> {
//!     let handle = private_storage::EntryHandle::<MyState>::new(b"MY_STATE_KEY");
//!     
//!     // Get or initialize with default
//!     let mut state_ref = handle.get_or_default()?;
//!     
//!     // Access and modify state through as_mut()
//!     let mut state_mut = state_ref.as_mut();
//!     state_mut.counter += 1;
//!     state_mut.data = "Hello, World!".to_string();
//!     
//!     // State is automatically saved when EntryMut is dropped
//!     Ok(())
//! }
//! ```

use std::marker::PhantomData;
use std::ops::{Deref, DerefMut};

use borsh::{self, BorshDeserialize, BorshSerialize};

use crate::env;

/// A handle for accessing private storage entries.
///
/// This type provides methods to read, write, and manage state in private storage.
/// It acts as a factory for creating `EntryRef` and `EntryMut` instances.
#[derive(Clone, Copy, Debug)]
pub struct EntryHandle<T> {
    key: &'static [u8],
    _phantom: PhantomData<T>,
}

impl<T> EntryHandle<T> {
    /// Create a new EntryHandle with the specified key.
    ///
    /// # Arguments
    ///
    /// * `key` - The storage key to use for this entry
    ///
    /// # Example
    ///
    /// ```rust
    /// use calimero_sdk::private_storage::EntryHandle;
    ///
    /// struct MyState {
    ///     counter: u32,
    /// }
    ///
    /// let handle = EntryHandle::<MyState>::new(b"my_state_key");
    /// ```
    pub const fn new(key: &'static [u8]) -> Self {
        Self {
            key,
            _phantom: PhantomData,
        }
    }

    /// Get the current state from storage.
    ///
    /// Returns `Ok(Some(EntryRef))` if the state exists, `Ok(None)` if it doesn't,
    /// or an error if deserialization fails.
    ///
    /// # Example
    ///
    /// ```rust
    /// use calimero_sdk::private_storage::EntryHandle;
    /// use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
    ///
    /// #[derive(BorshDeserialize, BorshSerialize, Debug)]
    /// struct MyState {
    ///     counter: u32,
    /// }
    ///
    /// fn example() -> calimero_sdk::app::Result<()> {
    ///     let handle = EntryHandle::<MyState>::new(b"my_key");
    ///     match handle.get()? {
    ///         Some(state) => println!("State exists: {:?}", *state),
    ///         None => println!("No state found"),
    ///     }
    ///     Ok(())
    /// }
    /// ```
    pub fn get(&self) -> crate::app::Result<Option<EntryRef<T>>>
    where
        T: BorshDeserialize,
    {
        // Use private storage functions (node-local, NOT synchronized)
        let Some(data) = env::private_storage_read(self.key) else {
            return Ok(None);
        };

        let state = T::try_from_slice(&data)
            .map_err(|e| crate::types::Error::msg(&format!("Failed to deserialize state: {e}")))?;

        Ok(Some(EntryRef {
            data: state,
            key: self.key,
            _phantom: PhantomData,
        }))
    }

    /// Get the state or initialize it with a custom function.
    ///
    /// If the state exists, it will be loaded. If not, it will be initialized
    /// using the provided function.
    ///
    /// # Arguments
    ///
    /// * `f` - Function to initialize the state if it doesn't exist
    ///
    /// # Example
    ///
    /// ```rust
    /// use calimero_sdk::private_storage::EntryHandle;
    /// use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
    ///
    /// #[derive(BorshDeserialize, BorshSerialize)]
    /// struct MyState {
    ///     counter: u32,
    ///     data: String,
    /// }
    ///
    /// fn example() -> calimero_sdk::app::Result<()> {
    ///     let handle = EntryHandle::<MyState>::new(b"my_key");
    ///     let _state = handle.get_or_init_with(|| MyState {
    ///         counter: 42,
    ///         data: "initial".to_string(),
    ///     })?;
    ///     Ok(())
    /// }
    /// ```
    pub fn get_or_init_with<F>(&self, f: F) -> crate::app::Result<EntryRef<T>>
    where
        T: BorshDeserialize + BorshSerialize,
        F: FnOnce() -> T,
    {
        if let Some(state) = self.get()? {
            Ok(state)
        } else {
            let initial_state = f();
            let entry = EntryRef {
                data: initial_state,
                key: self.key,
                _phantom: PhantomData,
            };
            entry.save()?;
            Ok(entry)
        }
    }

    /// Get the state or initialize it with the default value.
    ///
    /// If the state exists, it will be loaded. If not, it will be initialized
    /// using `T::default()`.
    ///
    /// # Example
    ///
    /// ```rust
    /// use calimero_sdk::private_storage::EntryHandle;
    /// use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
    ///
    /// #[derive(BorshDeserialize, BorshSerialize, Default)]
    /// struct MyState {
    ///     counter: u32,
    /// }
    ///
    /// fn example() -> calimero_sdk::app::Result<()> {
    ///     let handle = EntryHandle::<MyState>::new(b"my_key");
    ///     let _state = handle.get_or_default()?;
    ///     Ok(())
    /// }
    /// ```
    pub fn get_or_default(&self) -> crate::app::Result<EntryRef<T>>
    where
        T: BorshDeserialize + BorshSerialize + Default,
    {
        self.get_or_init_with(T::default)
    }

    /// Modify the state in place using a closure and persist the changes.
    ///
    /// Loads the current state (or initializes with `Default`), provides a mutable
    /// reference to it to the provided closure, then saves the updated state.
    ///
    /// This is equivalent to calling `get_or_default()`, then `as_mut()` to obtain
    /// an `EntryMut`, mutating the state, and letting it drop to persist.
    pub fn modify<F>(&self, f: F) -> crate::app::Result<()>
    where
        T: BorshDeserialize + BorshSerialize + Default,
        F: FnOnce(&mut T),
    {
        let mut entry = self.get_or_default()?;
        {
            let mut entry_mut = entry.as_mut();
            f(&mut entry_mut);
        }
        // Drop of EntryMut writes; explicit save keeps semantics clear.
        let _ = entry.save()?;
        Ok(())
    }
}

/// A read-only reference to private storage state.
///
/// This type provides read access to the stored state and can be converted
/// to a mutable reference for modifications.
#[derive(Debug)]
pub struct EntryRef<T> {
    data: T,
    key: &'static [u8],
    _phantom: PhantomData<T>,
}

impl<T> EntryRef<T> {
    /// Convert this read-only reference to a mutable reference.
    ///
    /// This allows you to modify the state. The changes will be automatically
    /// saved when the `EntryMut` is dropped.
    ///
    /// # Example
    ///
    /// ```rust
    /// use calimero_sdk::private_storage::EntryHandle;
    /// use calimero_sdk::borsh::{BorshDeserialize, BorshSerialize};
    ///
    /// #[derive(BorshDeserialize, BorshSerialize, Default)]
    /// struct MyState {
    ///     counter: u32,
    /// }
    ///
    /// fn example() -> calimero_sdk::app::Result<()> {
    ///     let handle = EntryHandle::<MyState>::new(b"my_key");
    ///     let mut state_ref = handle.get_or_default()?;
    ///     let mut state_mut = state_ref.as_mut();
    ///     state_mut.counter += 1;
    ///     // Changes are automatically saved when state_mut is dropped
    ///     Ok(())
    /// }
    /// ```
    pub fn as_mut(&mut self) -> EntryMut<'_, T>
    where
        T: BorshSerialize,
    {
        EntryMut {
            data: &mut self.data,
            key: self.key,
            _phantom: PhantomData,
        }
    }

    /// Save the current state to storage.
    ///
    /// This is called automatically when `EntryMut` is dropped, but can be
    /// called manually if needed.
    pub fn save(&self) -> crate::app::Result<()>
    where
        T: BorshSerialize,
    {
        let data = borsh::to_vec(&self.data)
            .map_err(|e| crate::types::Error::msg(&format!("Failed to serialize state: {e}")))?;

        // Use private storage functions (node-local, NOT synchronized)
        let _ = env::private_storage_write(self.key, &data);
        Ok(())
    }
}

impl<T> Deref for EntryRef<T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

/// A mutable reference to private storage state.
///
/// This type provides mutable access to the stored state and automatically
/// saves changes when dropped.
#[derive(Debug)]
pub struct EntryMut<'a, T: BorshSerialize> {
    data: &'a mut T,
    key: &'static [u8],
    _phantom: PhantomData<&'a T>,
}

impl<T: BorshSerialize> Deref for EntryMut<'_, T> {
    type Target = T;

    fn deref(&self) -> &Self::Target {
        self.data
    }
}

impl<T: BorshSerialize> DerefMut for EntryMut<'_, T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.data
    }
}

impl<T: BorshSerialize> Drop for EntryMut<'_, T> {
    fn drop(&mut self) {
        let data = borsh::to_vec(self.data).unwrap_or_else(|err| {
            env::panic_str(&format!(
                "Failed to serialize private storage state on drop: {err}"
            ));
        });

        // Use private storage functions (node-local, NOT synchronized)
        let wrote = env::private_storage_write(self.key, &data);
        if !wrote {
            env::panic_str("Failed to write private storage state on drop");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use borsh::{BorshDeserialize, BorshSerialize};

    #[derive(Default, BorshSerialize, BorshDeserialize, Debug, PartialEq)]
    struct TestState {
        counter: u32,
        message: String,
    }

    const TEST_KEY: &[u8] = b"test";

    #[test]
    fn test_entry_handle_creation() {
        let handle = EntryHandle::<TestState>::new(TEST_KEY);
        // This should compile without issues
        assert_eq!(handle.key, b"test");
    }

    #[test]
    fn test_entry_handle_key_access() {
        let handle = EntryHandle::<TestState>::new(b"hello");
        assert_eq!(handle.key, b"hello");
    }
}
