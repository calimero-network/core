//! Tests for SDK macros interacting with other Rust attributes
//!
//! This file tests that SDK macros work correctly when combined with
//! standard Rust attributes like #[derive], #[cfg], #[allow], etc.

use calimero_sdk::app;

// Test combining app::state with derive attributes
#[app::state]
#[derive(Clone)]
struct StateWithDerive {
    value: String,
}

#[app::logic]
impl StateWithDerive {
    pub fn get_value(&self) -> &str {
        &self.value
    }

    pub fn clone_self(&self) -> Self {
        self.clone()
    }
}

// Test combining app::state with multiple derive attributes
#[app::state]
#[derive(Clone, Default)]
struct StateWithMultipleDerive {
    count: u64,
    items: Vec<String>,
}

#[app::logic]
impl StateWithMultipleDerive {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_count(&self) -> u64 {
        self.count
    }
}

// Test doc attributes on state
/// This is a documented state struct
#[app::state]
struct DocumentedState {
    /// The main data field
    data: String,
}

#[app::logic]
impl DocumentedState {
    /// Gets the data
    pub fn get_data(&self) -> &str {
        &self.data
    }

    /// Sets the data
    pub fn set_data(&mut self, value: String) {
        self.data = value;
    }
}

// Test allow/deny attributes on logic methods
#[app::state]
struct StateWithLintAttributes;

#[app::logic]
impl StateWithLintAttributes {
    #[allow(unused_variables)]
    pub fn with_allow(&self, unused: String) {}

    #[allow(clippy::needless_return)]
    pub fn with_clippy_allow(&self) -> u64 {
        return 42;
    }
}

// Test repr attributes don't interfere
#[repr(C)]
#[app::state]
struct ReprCState {
    x: i32,
    y: i32,
}

#[app::logic]
impl ReprCState {
    pub fn get_x(&self) -> i32 {
        self.x
    }

    pub fn get_y(&self) -> i32 {
        self.y
    }
}

// Test cfg attributes work with state (basic)
// Using #[cfg(test)] which is always true during test compilation
#[app::state]
struct ConditionalState {
    #[cfg(test)]
    conditional_field: String,
    always_present: u64,
}

#[app::logic]
impl ConditionalState {
    pub fn get_always_present(&self) -> u64 {
        self.always_present
    }

    #[cfg(test)]
    pub fn get_conditional(&self) -> &str {
        &self.conditional_field
    }
}

// Test must_use attribute on methods
#[app::state]
struct MustUseState {
    value: u64,
}

#[app::logic]
impl MustUseState {
    #[must_use]
    pub fn compute(&self) -> u64 {
        self.value * 2
    }
}

// Test inline attributes
#[app::state]
struct InlineState {
    data: String,
}

#[app::logic]
impl InlineState {
    #[inline]
    pub fn get_inline(&self) -> &str {
        &self.data
    }

    #[inline(always)]
    pub fn get_inline_always(&self) -> &str {
        &self.data
    }
}

// Test deprecated attribute
#[app::state]
struct DeprecatedMethodState {
    value: String,
}

#[app::logic]
impl DeprecatedMethodState {
    #[deprecated(note = "Use get_new_value instead")]
    pub fn get_old_value(&self) -> &str {
        &self.value
    }

    pub fn get_new_value(&self) -> &str {
        &self.value
    }
}

fn main() {}
